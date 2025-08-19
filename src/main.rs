use anyhow::{anyhow, Result};
use bs58;
use bytemuck::{Pod, Zeroable};
use clap::Parser;
use hex::ToHex;
use pollster::block_on;
use rayon::prelude::*;
use ripemd::Ripemd160;
use secp256k1::{PublicKey, Secp256k1, SecretKey};
use sha2::{Digest, Sha256};
use std::borrow::Cow;
use std::cmp::Ordering;
use std::mem::size_of;
use wgpu::{util::DeviceExt, BufferUsages};

/// Search for a P2PKH address in a hex keyspace using the GPU to generate candidates.
#[derive(Parser, Debug)]
#[command(name = "gpu-keyspace-search")]
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

    // GPU init
    let gpu = block_on(GpuSeq::new())?;

    // Scan loop
    let mut cur = start_words;
    let batch = args.batch.max(1);
    let secp = Secp256k1::new();

    loop {
        // remaining = end - cur + 1
        let (rem, borrow) = sub_u256_le(&end_words, &cur);
        if borrow != 0 {
            break; // cur > end
        }
        let remaining_u64 = low64(&rem).saturating_add(1); // inclusive
        if remaining_u64 == 0 {
            break;
        }

        let this_batch = remaining_u64.min(batch as u64) as u32;

        // 1) Ask GPU to write cur..cur+this_batch-1 into a storage buffer.
        let le_bytes = block_on(gpu.generate_seq(cur, this_batch))?;

        // 2) CPU parallel: derive pubkey, hash160, compare to target
        let pos = le_bytes
            .par_chunks_exact(32)
            .position_any(|le32| {
                // Convert LE 32 bytes -> BE 32 bytes (secp expects big-endian)
                let mut be = [0u8; 32];
                for i in 0..32 {
                    be[i] = le32[31 - i];
                }
                // Zero is invalid
                if be.iter().all(|&b| b == 0) {
                    return false;
                }
                let sk = match SecretKey::from_slice(&be) {
                    Ok(s) => s,
                    Err(_) => return false,
                };
                let pk = PublicKey::from_secret_key(&secp, &sk);
                let pkc = pk.serialize(); // 33 bytes, compressed

                let h160 = hash160(&pkc);
                h160 == target_h160
            });

        if let Some(p) = pos {
            // Recreate the winning key and print details
            let winner_le = &le_bytes[p * 32..p * 32 + 32];
            let mut be = [0u8; 32];
            for i in 0..32 {
                be[i] = winner_le[31 - i];
            }
            let sk = SecretKey::from_slice(&be).expect("valid secret");
            let pk = PublicKey::from_secret_key(&secp, &sk);
            let pkc = pk.serialize();
            let address = p2pkh_from_pubkey_compressed(&pkc);
            let wif = wif_from_secret(&sk);

            println!("FOUND!");
            println!("address  : {address}");
            println!("wif      : {wif}");
            println!("priv_hex : {}", be.encode_hex::<String>());

            if args.verbose {
                println!("pubkey   : {}", pkc.encode_hex::<String>());
            }
            return Ok(());
        }

        // 3) Advance to next batch
        cur = add_small_u256_le(cur, this_batch as u64);

        // Stop if we just finished the last range
        if cmp_u256_le(&cur, &end_words) == Ordering::Greater {
            break;
        }
    }

    println!("Not found in the given range.");
    Ok(())
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
}

impl GpuSeq {
    async fn new() -> Result<Self> {
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

        let shader_src = include_str!("../shaders/seq.wgsl");
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("seq.wgsl"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(shader_src)),
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

        Ok(Self {
            device,
            queue,
            pipeline,
            bind_layout,
        })
    }

    /// Generate `n` sequential 256-bit numbers starting at `start_le` (LE words).
    async fn generate_seq(&self, start_le: [u32; 8], n: u32) -> Result<Vec<u8>> {
        let out_u32_len = (n as usize) * 8;
        let out_size_bytes = (out_u32_len * size_of::<u32>()) as u64;

        let out_storage = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("out_storage"),
            size: out_size_bytes,
            usage: BufferUsages::STORAGE | BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });

        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: out_size_bytes,
            usage: BufferUsages::MAP_READ | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

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
                    resource: out_storage.as_entire_binding(),
                },
            ],
        });

        // Dispatch
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("encoder") });
        {
            let mut cpass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("seq pass"),
                timestamp_writes: None,
            });
            cpass.set_pipeline(&self.pipeline);
            cpass.set_bind_group(0, &bind_group, &[]);
            const WG: u32 = 256;
            let groups = (n + (WG - 1)) / WG;
            cpass.dispatch_workgroups(groups, 1, 1);
        }
        encoder.copy_buffer_to_buffer(&out_storage, 0, &readback, 0, out_size_bytes);
        self.queue.submit(Some(encoder.finish()));

        // Map + read
        let slice = readback.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        self.device.poll(wgpu::Maintain::Wait);
        let data = slice.get_mapped_range();
        let mut bytes = vec![0u8; out_size_bytes as usize];
        bytes.copy_from_slice(&data);
        drop(data);
        readback.unmap();

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
    let s = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")).unwrap_or(s);
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
    for i in 2..8 {
        if carry == 0 { break; }
        let (ri, ci) = a[i].overflowing_add(carry);
        a[i] = ri;
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

