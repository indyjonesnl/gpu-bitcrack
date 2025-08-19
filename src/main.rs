use anyhow::{Result, anyhow};
use bytemuck::{Pod, Zeroable};
use clap::Parser;
use futures::channel::oneshot;
use hex::ToHex;
use pollster::block_on;
use ripemd::Ripemd160;
use secp256k1::{PublicKey, Secp256k1, SecretKey};
use sha2::{Digest, Sha256};
use std::borrow::Cow;
use std::cmp::Ordering;
use std::mem::size_of;
use wgpu::{BufferSlice, BufferUsages, util::DeviceExt};

/// Search for a P2PKH address in a hex keyspace using the GPU to generate candidates.
#[derive(Parser, Debug)]
#[command(name = "gpu-bitcrack")]
#[command(about = "Find a Bitcoin P2PKH address within a private-key hex range using GPU+CPU")]
struct Args {
    /// Keyspace as START:END in hex (inclusive), e.g. 1000000:1ffffff
    keyspace: String,

    /// Target P2PKH address (Base58, starts with '1')
    target: String,

    /// Batch size (candidates per GPU dispatch)
    #[arg(long, default_value_t = 1_000_000)]
    batch: u32,

    /// Print extra details if found
    #[arg(long)]
    verbose: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();
    block_on(run(args))
}

async fn run(args: Args) -> Result<()> {
    // Parse keyspace
    let (start_str, end_str) = args
        .keyspace
        .split_once(':')
        .ok_or_else(|| anyhow!("--keyspace must be START:END hex"))?;
    let start_words = hex_to_u256_le_words(start_str)?;
    let end_words = hex_to_u256_le_words(end_str)?;
    if cmp_u256_le(&start_words, &end_words) == Ordering::Greater {
        return Err(anyhow!("keyspace start > end"));
    }

    // Decode target address -> HASH160
    let target_h160 = decode_p2pkh_to_hash160(&args.target)?;

    // Batch size and GPU init
    let batch = args.batch.max(1);
    let mut gpu = GpuSeq::new(batch).await?;
    let mut cur = start_words;
    let secp = Secp256k1::new();

    loop {
        let (rem, borrow) = sub_u256_le(&end_words, &cur);
        let remaining_u64 = low64(&rem).saturating_add(1);
        if borrow != 0 || remaining_u64 == 0 {
            break;
        }

        let n = remaining_u64.min(batch as u64) as u32;
        let (_, out_recv, hits_recv) = gpu.dispatch_and_map(cur, n, 0)?;

        gpu.poll();
        out_recv.await.unwrap()?;
        hits_recv.await.unwrap()?;
        gpu.unmap(0);

        {
            let slice = gpu.hits_slice();
            let data = slice.get_mapped_range();
            let hits: &[u32] = bytemuck::cast_slice(&data);
            let count = hits[0].min(gpu.max_hits);
            for i in 0..count as usize {
                let idx = hits[i + 1];
                if verify_hit(cur, idx, &secp, &target_h160, args.verbose) {
                    gpu.unmap_hits();
                    return Ok(());
                }
            }
        }
        gpu.unmap_hits();
        cur = add_small_u256_le(cur, n as u64);
    }

    println!("Not found in the given range.");
    Ok(())
}

fn verify_hit(
    start: [u32; 8],
    idx: u32,
    secp: &Secp256k1<secp256k1::All>,
    target_h160: &[u8; 20],
    verbose: bool,
) -> bool {
    let candidate = add_small_u256_le(start, idx as u64);
    let mut le = [0u8; 32];
    for i in 0..8 {
        le[i * 4..i * 4 + 4].copy_from_slice(&candidate[i].to_le_bytes());
    }
    let mut be = [0u8; 32];
    for i in 0..32 {
        be[i] = le[31 - i];
    }
    if be.iter().all(|&b| b == 0) {
        return false;
    }
    let sk = match SecretKey::from_slice(&be) {
        Ok(s) => s,
        Err(_) => return false,
    };
    let pk = PublicKey::from_secret_key(secp, &sk);
    let pkc = pk.serialize();
    let h160 = hash160(&pkc);
    if h160 != *target_h160 {
        return false;
    }
    let address = p2pkh_from_pubkey_compressed(&pkc);
    let wif = wif_from_secret(&sk);
    println!("FOUND!");
    println!("address  : {address}");
    println!("wif      : {wif}");
    println!("priv_hex : {}", be.encode_hex::<String>());
    if verbose {
        println!("pubkey   : {}", pkc.encode_hex::<String>());
    }
    true
}

/* --------------------------- GPU sequence writer -------------------------- */

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Params {
    start0: u32,
    start1: u32,
    start2: u32,
    start3: u32,
    start4: u32,
    start5: u32,
    start6: u32,
    start7: u32,
    n: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
}

struct GpuSeq {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::ComputePipeline,
    bind_layout: wgpu::BindGroupLayout,
    out_storage: [wgpu::Buffer; 2],
    readback: [wgpu::Buffer; 2],
    hits_storage: wgpu::Buffer,
    hits_readback: wgpu::Buffer,
    capacity: u32,
    max_hits: u32,
}

impl GpuSeq {
    async fn new(max_batch: u32) -> Result<Self> {
        let instance = wgpu::Instance::default();
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: None,
                force_fallback_adapter: false,
            })
            .await
            .ok_or_else(|| anyhow!("No suitable GPU adapter found"))?;

        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("Device"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::downlevel_defaults(),
                },
                None,
            )
            .await?;

        let shader_src = [
            include_str!("../shaders/hits.wgsl"),
            include_str!("../shaders/seq.wgsl"),
        ]
        .join("\n");
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("seq.wgsl"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(&shader_src)),
        });

        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("bind layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("pipeline layout"),
            bind_group_layouts: &[&bind_layout],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("compute pipeline"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: "main",
            compilation_options: Default::default(),
        });

        let capacity = max_batch.max(1);
        let buf_size = (capacity as u64) * 32;
        let out_storage = [0, 1].map(|_| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("out_storage"),
                size: buf_size,
                usage: BufferUsages::STORAGE | BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            })
        });
        let readback = [0, 1].map(|_| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("readback"),
                size: buf_size,
                usage: BufferUsages::MAP_READ | BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        });

        let max_hits = 1024u32;
        let hits_buf_size = ((max_hits + 1) as u64) * 4;
        let hits_storage = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("hits_storage"),
            size: hits_buf_size,
            usage: BufferUsages::STORAGE | BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let hits_readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("hits_readback"),
            size: hits_buf_size,
            usage: BufferUsages::MAP_READ | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Ok(Self {
            device,
            queue,
            pipeline,
            bind_layout,
            out_storage,
            readback,
            hits_storage,
            hits_readback,
            capacity,
            max_hits,
        })
    }
    fn poll(&self) {
        self.device.poll(wgpu::Maintain::Wait);
    }

    fn unmap(&self, idx: usize) {
        self.readback[idx].unmap();
    }

    #[cfg(test)]
    fn slice(&self, idx: usize, size: u64) -> BufferSlice<'_> {
        self.readback[idx].slice(0..size)
    }

    fn unmap_hits(&self) {
        self.hits_readback.unmap();
    }

    fn hits_slice(&self) -> BufferSlice<'_> {
        let size = ((self.max_hits + 1) as u64) * 4;
        self.hits_readback.slice(0..size)
    }

    #[allow(clippy::type_complexity)]
    fn dispatch_and_map(
        &mut self,
        start_le: [u32; 8],
        n: u32,
        idx: usize,
    ) -> Result<(
        u64,
        oneshot::Receiver<Result<(), wgpu::BufferAsyncError>>,
        oneshot::Receiver<Result<(), wgpu::BufferAsyncError>>,
    )> {
        let out_u32_len = (n as usize) * 8;
        let out_size_bytes = (out_u32_len * size_of::<u32>()) as u64;

        if n > self.capacity {
            let new_size = (n as u64) * 32;
            self.out_storage = [0, 1].map(|_| {
                self.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("out_storage"),
                    size: new_size,
                    usage: BufferUsages::STORAGE | BufferUsages::COPY_SRC,
                    mapped_at_creation: false,
                })
            });
            self.readback = [0, 1].map(|_| {
                self.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("readback"),
                    size: new_size,
                    usage: BufferUsages::MAP_READ | BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                })
            });
            self.capacity = n;
        }

        let params = Params {
            start0: start_le[0],
            start1: start_le[1],
            start2: start_le[2],
            start3: start_le[3],
            start4: start_le[4],
            start5: start_le[5],
            start6: start_le[6],
            start7: start_le[7],
            n,
            _pad0: 0,
            _pad1: 0,
            _pad2: 0,
        };

        let params_buf = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("params"),
                contents: bytemuck::bytes_of(&params),
                usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            });

        self.queue
            .write_buffer(&self.hits_storage, 0, bytemuck::cast_slice(&[0u32]));

        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("bind group"),
            layout: &self.bind_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: params_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: self.out_storage[idx].as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.hits_storage.as_entire_binding(),
                },
            ],
        });

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("encoder"),
            });
        {
            let mut cpass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("seq pass"),
                timestamp_writes: None,
            });
            cpass.set_pipeline(&self.pipeline);
            cpass.set_bind_group(0, &bind_group, &[]);
            const WG: u32 = 256;
            let groups = n.div_ceil(WG);
            cpass.dispatch_workgroups(groups, 1, 1);
        }
        encoder.copy_buffer_to_buffer(
            &self.out_storage[idx],
            0,
            &self.readback[idx],
            0,
            out_size_bytes,
        );
        let hits_size = ((self.max_hits + 1) as u64) * 4;
        encoder.copy_buffer_to_buffer(&self.hits_storage, 0, &self.hits_readback, 0, hits_size);
        self.queue.submit(Some(encoder.finish()));

        let slice = self.readback[idx].slice(0..out_size_bytes);
        let hits_slice = self.hits_readback.slice(0..hits_size);
        let (sender_out, receiver_out) = oneshot::channel();
        let (sender_hits, receiver_hits) = oneshot::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = sender_out.send(r);
        });
        hits_slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = sender_hits.send(r);
        });
        Ok((out_size_bytes, receiver_out, receiver_hits))
    }

    /// Convenience method used in tests to generate a batch synchronously.
    #[cfg(test)]
    async fn generate_seq(&mut self, start_le: [u32; 8], n: u32) -> Result<Vec<u8>> {
        let (out_size_bytes, out_recv, hits_recv) = self.dispatch_and_map(start_le, n, 0)?;
        self.poll();
        out_recv.await.unwrap()?;
        hits_recv.await.unwrap()?;
        {
            let slice = self.hits_slice();
            let _ = slice.get_mapped_range();
        }
        self.unmap_hits();
        let mut bytes = vec![0u8; out_size_bytes as usize];
        {
            let slice = self.slice(0, out_size_bytes);
            let data = slice.get_mapped_range();
            bytes.copy_from_slice(&data);
        }
        self.unmap(0);
        Ok(bytes)
    }
}

/* ----------------------------- Utility logic ------------------------------ */

fn decode_p2pkh_to_hash160(addr: &str) -> Result<[u8; 20]> {
    let raw = bs58::decode(addr).into_vec()?;
    if raw.len() < 25 {
        return Err(anyhow!("Invalid Base58Check length"));
    }
    let (payload, checksum) = raw.split_at(raw.len() - 4);
    let checksum_expected = Sha256::digest(Sha256::digest(payload));
    if &checksum_expected[..4] != checksum {
        return Err(anyhow!("Invalid Base58Check checksum"));
    }
    if payload[0] != 0x00 {
        return Err(anyhow!("Only P2PKH mainnet (version 0x00) is supported"));
    }
    if payload.len() != 1 + 20 {
        return Err(anyhow!("Invalid P2PKH payload length"));
    }
    let mut h = [0u8; 20];
    h.copy_from_slice(&payload[1..]);
    Ok(h)
}

fn hash160(data: &[u8]) -> [u8; 20] {
    let sha = Sha256::digest(data);
    let mut rip = Ripemd160::new();
    rip.update(sha);
    let out = rip.finalize();
    let mut h = [0u8; 20];
    h.copy_from_slice(&out);
    h
}

fn p2pkh_from_pubkey_compressed(pk33: &[u8; 33]) -> String {
    let h = hash160(pk33);
    let mut payload = Vec::with_capacity(1 + 20 + 4);
    payload.push(0x00);
    payload.extend_from_slice(&h);
    base58check(&payload)
}

fn wif_from_secret(sk: &SecretKey) -> String {
    let mut payload = Vec::with_capacity(1 + 32 + 1 + 4);
    payload.push(0x80);
    payload.extend_from_slice(&sk.secret_bytes());
    payload.push(0x01); // compressed
    base58check(&payload)
}

fn base58check(payload: &[u8]) -> String {
    let c = Sha256::digest(Sha256::digest(payload));
    let mut v = payload.to_vec();
    v.extend_from_slice(&c[..4]);
    bs58::encode(v).into_string()
}

/* ----------------------------- 256-bit helpers ---------------------------- */

fn hex_to_u256_le_words(s: &str) -> Result<[u32; 8]> {
    let s = s.trim();
    // strip 0x/0X if present
    let s = s
        .strip_prefix("0x")
        .or_else(|| s.strip_prefix("0X"))
        .unwrap_or(s);
    // allow underscores in hex for readability
    let mut s = s.replace('_', "");

    if s.is_empty() {
        return Err(anyhow!("empty hex"));
    }
    // hex::decode needs an even number of nibbles
    if s.len() % 2 == 1 {
        s.insert(0, '0'); // left-pad one zero to make it even-length
    }

    let bytes = hex::decode(&s)?;
    if bytes.len() > 32 {
        return Err(anyhow!("hex too large (>256 bits)"));
    }

    // big-endian -> fixed 32 bytes
    let mut be = [0u8; 32];
    be[32 - bytes.len()..].copy_from_slice(&bytes);

    be_to_le_words(&be)
}

fn be_to_le_words(be32: &[u8; 32]) -> Result<[u32; 8]> {
    // Convert 32 big-endian bytes into 8 little-endian u32 limbs
    // Limb 0 is least significant (little-endian word order)
    let mut w = [0u32; 8];
    for i in 0..8 {
        let j = i * 4;
        let limb_be = u32::from_be_bytes([be32[j], be32[j + 1], be32[j + 2], be32[j + 3]]);
        w[7 - i] = limb_be;
    }
    Ok(w)
}

fn add_small_u256_le(mut a: [u32; 8], add: u64) -> [u32; 8] {
    let add0 = (add & 0xFFFF_FFFF) as u32;
    let add1 = (add >> 32) as u32;

    // a[0] += low32(add)
    let (r0, c0) = a[0].overflowing_add(add0);
    a[0] = r0;

    // a[1] += high32(add) + carry0
    let (r1a, c1a) = a[1].overflowing_add(add1);
    let (r1, c1b) = r1a.overflowing_add(c0 as u32);
    a[1] = r1;

    // propagate any remaining carry (at most 1) upward
    let mut carry = (c1a as u32) + (c1b as u32);
    for ai in a.iter_mut().skip(2) {
        if carry == 0 {
            break;
        }
        let (ri, ci) = ai.overflowing_add(carry);
        *ai = ri;
        carry = ci as u32;
    }
    a
}

fn sub_u256_le(a: &[u32; 8], b: &[u32; 8]) -> ([u32; 8], u32) {
    // returns (a - b, borrow)
    let mut out = [0u32; 8];
    let mut borrow: u64 = 0;
    for i in 0..8 {
        let av = a[i] as u64;
        let bv = b[i] as u64;
        let (res, br) = sub_with_borrow(av, bv, borrow);
        out[i] = res as u32;
        borrow = br;
    }
    (out, borrow as u32)
}

fn sub_with_borrow(a: u64, b: u64, borrow_in: u64) -> (u64, u64) {
    let tmp = a.wrapping_sub(b).wrapping_sub(borrow_in);
    let borrow_out = ((a as u128) < ((b as u128) + (borrow_in as u128))) as u64;
    (tmp, borrow_out)
}

fn cmp_u256_le(a: &[u32; 8], b: &[u32; 8]) -> Ordering {
    for i in (0..8).rev() {
        if a[i] < b[i] {
            return Ordering::Less;
        } else if a[i] > b[i] {
            return Ordering::Greater;
        }
    }
    Ordering::Equal
}

fn low64(x: &[u32; 8]) -> u64 {
    (x[1] as u64) << 32 | (x[0] as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pollster::block_on;
    use serial_test::file_serial;
    use std::cmp::Ordering;

    #[test]
    fn be_to_le_words_converts_correctly() {
        let mut be = [0u8; 32];
        for (i, b) in be.iter_mut().enumerate() {
            *b = (i + 1) as u8;
        }
        let w = be_to_le_words(&be).expect("convert");
        assert_eq!(
            w,
            [
                0x1d1e_1f20,
                0x191a_1b1c,
                0x1516_1718,
                0x1112_1314,
                0x0d0e_0f10,
                0x090a_0b0c,
                0x0506_0708,
                0x0102_0304,
            ]
        );
    }

    #[test]
    fn hex_to_u256_le_words_handles_basic_cases() {
        assert_eq!(
            hex_to_u256_le_words("1").expect("hex"),
            [1, 0, 0, 0, 0, 0, 0, 0]
        );
        let words = hex_to_u256_le_words("abc").expect("hex");
        assert_eq!(words[0], 0x0abc);
        assert!(words[1..].iter().all(|&w| w == 0));
    }

    #[test]
    fn add_small_u256_le_propagates_carry() {
        let a = [u32::MAX, 0, 0, 0, 0, 0, 0, 0];
        let r = add_small_u256_le(a, 1);
        assert_eq!(r, [0, 1, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn sub_with_borrow_handles_underflow() {
        let (r, b) = sub_with_borrow(5, 3, 0);
        assert_eq!((r, b), (2, 0));
        let (r2, b2) = sub_with_borrow(3, 5, 0);
        assert_eq!(r2, u64::MAX - 1);
        assert_eq!(b2, 1);
    }

    #[test]
    fn sub_u256_le_borrow() {
        let (r, b) = sub_u256_le(&[5, 0, 0, 0, 0, 0, 0, 0], &[3, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(r, [2, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(b, 0);

        let (r2, b2) = sub_u256_le(&[0; 8], &[1, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(r2, [u32::MAX; 8]);
        assert_eq!(b2, 1);
    }

    #[test]
    fn cmp_u256_le_orders() {
        let a = [1, 0, 0, 0, 0, 0, 0, 0];
        let b = [2, 0, 0, 0, 0, 0, 0, 0];
        assert_eq!(cmp_u256_le(&a, &b), Ordering::Less);
        assert_eq!(cmp_u256_le(&b, &a), Ordering::Greater);
        let a2 = [1, 0, 0, 0, 0, 0, 0, 0];
        assert_eq!(cmp_u256_le(&a, &a2), Ordering::Equal);
    }

    #[test]
    fn low64_extracts_least_significant_bits() {
        let x = [0x89ab_cdef, 0x0123_4567, 0, 0, 0, 0, 0, 0];
        assert_eq!(low64(&x), 0x0123_4567_89ab_cdef);
    }

    #[test]
    fn hash160_matches_known_vector() {
        let h = hash160(b"hello");
        assert_eq!(
            h,
            [
                0xb6, 0xa9, 0xc8, 0xc2, 0x30, 0x72, 0x2b, 0x7c, 0x74, 0x83, 0x31, 0xa8, 0xb4, 0x50,
                0xf0, 0x55, 0x66, 0xdc, 0x7d, 0x0f,
            ]
        );
    }

    #[test]
    fn base58check_encodes_payload() {
        let payload = [0u8; 21];
        let s = base58check(&payload);
        assert_eq!(s, "1111111111111111111114oLvT2");
    }

    #[test]
    fn decode_p2pkh_to_hash160_known_address() {
        let h = decode_p2pkh_to_hash160("1CfZWK1QTQE3eS9qn61dQjV89KDjZzfNcv").unwrap();
        assert_eq!(
            h,
            [
                0x7f, 0xf4, 0x53, 0x03, 0x77, 0x4e, 0xf7, 0xa5, 0x2f, 0xff, 0xd8, 0x01, 0x19, 0x81,
                0x03, 0x4b, 0x25, 0x8c, 0xb8, 0x6b,
            ]
        );
    }

    #[test]
    fn p2pkh_from_pubkey_compressed_known() {
        let pk_bytes =
            hex::decode("0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798")
                .unwrap();
        let mut pk = [0u8; 33];
        pk.copy_from_slice(&pk_bytes);
        let addr = p2pkh_from_pubkey_compressed(&pk);
        assert_eq!(addr, "1BgGZ9tcN4rm9KBzDn7KprQz87SZ26SAMH");
    }

    #[test]
    fn wif_from_secret_known() {
        let mut b = [0u8; 32];
        b[31] = 1;
        let sk = SecretKey::from_slice(&b).unwrap();
        let wif = wif_from_secret(&sk);
        assert_eq!(wif, "KwDiBf89QgGbjEhKnhXJuH7LrciVrZi3qYjgd9M7rFU73sVHnoWn");
    }

    #[test]
    fn verify_hit_finds_secret_one() {
        let start = [1u32, 0, 0, 0, 0, 0, 0, 0];
        let target = decode_p2pkh_to_hash160("1BgGZ9tcN4rm9KBzDn7KprQz87SZ26SAMH").expect("addr");
        let secp = Secp256k1::new();
        assert!(verify_hit(start, 0, &secp, &target, false));
    }

    #[test]
    #[file_serial(gpu)]
    #[ignore]
    fn gpu_seq_resizes() {
        let mut gpu = block_on(GpuSeq::new(1)).expect("gpu init");
        let out = block_on(gpu.generate_seq([0; 8], 1)).expect("seq");
        assert_eq!(out.len(), 32);
        let out2 = block_on(gpu.generate_seq([0; 8], 2)).expect("seq");
        assert_eq!(out2.len(), 64);
    }
}
