struct Params {
  start0 : u32,
  start1 : u32,
  start2 : u32,
  start3 : u32,
  start4 : u32,
  start5 : u32,
  start6 : u32,
  start7 : u32,
  n      : u32,
  _pad0  : u32,
  _pad1  : u32,
  _pad2  : u32
};

@group(0) @binding(0)
var<uniform> params : Params;

@group(0) @binding(1)
var<storage, read_write> outbuf : array<u32>;

fn add_with_carry(a: u32, b: u32, carry_in: u32) -> vec2<u32> {
  let sum1 = a + b;
  let carry1 = select(0u, 1u, sum1 < b);
  let sum2 = sum1 + carry_in;
  let carry2 = select(0u, 1u, sum2 < sum1);
  return vec2<u32>(sum2, carry1 + carry2);
}

@compute @workgroup_size(256)
fn main(@builtin(global_invocation_id) gid : vec3<u32>) {
  let idx = gid.x;
  if (idx >= params.n) { return; }

  // Start limbs in LE
  var s = array<u32, 8>(
    params.start0, params.start1, params.start2, params.start3,
    params.start4, params.start5, params.start6, params.start7
  );

  // Add idx (fits u32) to 256-bit start
  var carry : u32 = idx;
  var c : u32 = 0u;

  var r0 = add_with_carry(s[0], carry, 0u);
  s[0] = r0.x; c = r0.y;

  for (var i: u32 = 1u; i < 8u; i = i + 1u) {
    let r = add_with_carry(s[i], 0u, c);
    s[i] = r.x; c = r.y;
  }

  let base = idx * 8u;
  outbuf[base + 0u] = s[0];
  outbuf[base + 1u] = s[1];
  outbuf[base + 2u] = s[2];
  outbuf[base + 3u] = s[3];
  outbuf[base + 4u] = s[4];
  outbuf[base + 5u] = s[5];
  outbuf[base + 6u] = s[6];
  outbuf[base + 7u] = s[7];

  // Placeholder: record the first index as a hit for demonstration.
  if (idx == 0u) {
    let pos = atomicAdd(&hits.count, 1u);
    if (pos < MAX_HITS) {
      hits.idx[pos] = idx;
    }
  }
}
