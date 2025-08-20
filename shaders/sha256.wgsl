struct Params {
    n: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
};

@group(0) @binding(0) var<storage, read> input: array<u32>;
@group(0) @binding(1) var<storage, read_write> output: array<u32>;
@group(0) @binding(2) var<uniform> params: Params;

fn rotr(x: u32, n: u32) -> u32 {
    return (x >> n) | (x << (32u - n));
}
fn ch(x: u32, y: u32, z: u32) -> u32 { return (x & y) ^ ((~x) & z); }
fn maj(x: u32, y: u32, z: u32) -> u32 { return (x & y) ^ (x & z) ^ (y & z); }
fn bsig0(x: u32) -> u32 { return rotr(x, 2u) ^ rotr(x, 13u) ^ rotr(x, 22u); }
fn bsig1(x: u32) -> u32 { return rotr(x, 6u) ^ rotr(x, 11u) ^ rotr(x, 25u); }
fn ssig0(x: u32) -> u32 { return rotr(x, 7u) ^ rotr(x, 18u) ^ (x >> 3u); }
fn ssig1(x: u32) -> u32 { return rotr(x, 17u) ^ rotr(x, 19u) ^ (x >> 10u); }


@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if idx >= params.n { return; }
    var w: array<u32, 64>;
    for (var i: u32 = 0u; i < 16u; i = i + 1u) {
        w[i] = 0u;
    }
    let base = idx * 9u;
    for (var j: u32 = 0u; j < 33u; j = j + 1u) {
        let word = input[base + j / 4u];
        let shift_src = (j % 4u) * 8u;
        let byte = (word >> shift_src) & 0xffu;
        let wi = j / 4u;
        let shift_dst = 24u - (j % 4u) * 8u;
        w[wi] = w[wi] | (byte << shift_dst);
    }
    w[8] = w[8] | (0x80u << 16u);
    w[15] = 264u;
    var k: array<u32, 64> = array<u32, 64>(
        0x428a2f98u, 0x71374491u, 0xb5c0fbcfu, 0xe9b5dba5u,
        0x3956c25bu, 0x59f111f1u, 0x923f82a4u, 0xab1c5ed5u,
        0xd807aa98u, 0x12835b01u, 0x243185beu, 0x550c7dc3u,
        0x72be5d74u, 0x80deb1feu, 0x9bdc06a7u, 0xc19bf174u,
        0xe49b69c1u, 0xefbe4786u, 0x0fc19dc6u, 0x240ca1ccu,
        0x2de92c6fu, 0x4a7484aau, 0x5cb0a9dcu, 0x76f988dau,
        0x983e5152u, 0xa831c66du, 0xb00327c8u, 0xbf597fc7u,
        0xc6e00bf3u, 0xd5a79147u, 0x06ca6351u, 0x14292967u,
        0x27b70a85u, 0x2e1b2138u, 0x4d2c6dfcu, 0x53380d13u,
        0x650a7354u, 0x766a0abbu, 0x81c2c92eu, 0x92722c85u,
        0xa2bfe8a1u, 0xa81a664bu, 0xc24b8b70u, 0xc76c51a3u,
        0xd192e819u, 0xd6990624u, 0xf40e3585u, 0x106aa070u,
        0x19a4c116u, 0x1e376c08u, 0x2748774cu, 0x34b0bcb5u,
        0x391c0cb3u, 0x4ed8aa4au, 0x5b9cca4fu, 0x682e6ff3u,
        0x748f82eeu, 0x78a5636fu, 0x84c87814u, 0x8cc70208u,
        0x90befffau, 0xa4506cebu, 0xbef9a3f7u, 0xc67178f2u,
    );
    for (var t: u32 = 16u; t < 64u; t = t + 1u) {
        w[t] = ssig1(w[t-2u]) + w[t-7u] + ssig0(w[t-15u]) + w[t-16u];
    }
    var a: u32 = 0x6a09e667u;
    var b: u32 = 0xbb67ae85u;
    var c: u32 = 0x3c6ef372u;
    var d: u32 = 0xa54ff53au;
    var e: u32 = 0x510e527fu;
    var f: u32 = 0x9b05688cu;
    var g: u32 = 0x1f83d9abu;
    var h: u32 = 0x5be0cd19u;
    for (var t: u32 = 0u; t < 64u; t = t + 1u) {
        let T1 = h + bsig1(e) + ch(e,f,g) + k[t] + w[t];
        let T2 = bsig0(a) + maj(a,b,c);
        h = g;
        g = f;
        f = e;
        e = d + T1;
        d = c;
        c = b;
        b = a;
        a = T1 + T2;
    }
    let out_base = idx * 8u;
    output[out_base + 0u] = a + 0x6a09e667u;
    output[out_base + 1u] = b + 0xbb67ae85u;
    output[out_base + 2u] = c + 0x3c6ef372u;
    output[out_base + 3u] = d + 0xa54ff53au;
    output[out_base + 4u] = e + 0x510e527fu;
    output[out_base + 5u] = f + 0x9b05688cu;
    output[out_base + 6u] = g + 0x1f83d9abu;
    output[out_base + 7u] = h + 0x5be0cd19u;
}
