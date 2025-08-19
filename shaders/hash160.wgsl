
struct HashParams {
  target0: u32;
  target1: u32;
  target2: u32;
  target3: u32;
  target4: u32;
  n: u32;
  _pad0: u32;
  _pad1: u32;
  _pad2: u32;
};

@group(0) @binding(0)
var<uniform> params : HashParams;

@group(0) @binding(1)
var<storage, read> inbuf : array<u32>;

@group(0) @binding(2)
var<storage, read_write> hits : array<u32>;

fn rotr(x: u32, n: u32) -> u32 {
  return (x >> n) | (x << (32u - n));
}

fn sha256_transform(msg: array<u32,16>) -> array<u32,8> {
  var k = array<u32,64>(
    0x428a2f98u,0x71374491u,0xb5c0fbcfu,0xe9b5dba5u,0x3956c25bu,0x59f111f1u,0x923f82a4u,0xab1c5ed5u,
    0xd807aa98u,0x12835b01u,0x243185beu,0x550c7dc3u,0x72be5d74u,0x80deb1feu,0x9bdc06a7u,0xc19bf174u,
    0xe49b69c1u,0xefbe4786u,0x0fc19dc6u,0x240ca1ccu,0x2de92c6fu,0x4a7484aau,0x5cb0a9dcu,0x76f988dau,
    0x983e5152u,0xa831c66du,0xb00327c8u,0xbf597fc7u,0xc6e00bf3u,0xd5a79147u,0x06ca6351u,0x14292967u,
    0x27b70a85u,0x2e1b2138u,0x4d2c6dfcu,0x53380d13u,0x650a7354u,0x766a0abbu,0x81c2c92eu,0x92722c85u,
    0xa2bfe8a1u,0xa81a664bu,0xc24b8b70u,0xc76c51a3u,0xd192e819u,0xd6990624u,0xf40e3585u,0x106aa070u,
    0x19a4c116u,0x1e376c08u,0x2748774cu,0x34b0bcb5u,0x391c0cb3u,0x4ed8aa4au,0x5b9cca4fu,0x682e6ff3u,
    0x748f82eeu,0x78a5636fu,0x84c87814u,0x8cc70208u,0x90befffau,0xa4506cebu,0xbef9a3f7u,0xc67178f2u);

  var w = array<u32,64>();
  for (var i:u32=0u;i<16u;i=i+1u){
    w[i]=msg[i];
  }
  for (var i:u32=16u;i<64u;i=i+1u){
    let s0 = rotr(w[i-15u],7u) ^ rotr(w[i-15u],18u) ^ (w[i-15u] >> 3u);
    let s1 = rotr(w[i-2u],17u) ^ rotr(w[i-2u],19u) ^ (w[i-2u] >> 10u);
    w[i] = w[i-16u] + s0 + w[i-7u] + s1;
  }

  var a:u32 = 0x6a09e667u;
  var b:u32 = 0xbb67ae85u;
  var c:u32 = 0x3c6ef372u;
  var d:u32 = 0xa54ff53au;
  var e:u32 = 0x510e527fu;
  var f:u32 = 0x9b05688cu;
  var g:u32 = 0x1f83d9abu;
  var h:u32 = 0x5be0cd19u;

  for (var i:u32=0u;i<64u;i=i+1u){
    let S1 = rotr(e,6u) ^ rotr(e,11u) ^ rotr(e,25u);
    let ch = (e & f) ^ ((~e) & g);
    let temp1 = h + S1 + ch + k[i] + w[i];
    let S0 = rotr(a,2u) ^ rotr(a,13u) ^ rotr(a,22u);
    let maj = (a & b) ^ (a & c) ^ (b & c);
    let temp2 = S0 + maj;

    h = g;
    g = f;
    f = e;
    e = d + temp1;
    d = c;
    c = b;
    b = a;
    a = temp1 + temp2;
  }

  a = a + 0x6a09e667u;
  b = b + 0xbb67ae85u;
  c = c + 0x3c6ef372u;
  d = d + 0xa54ff53au;
  e = e + 0x510e527fu;
  f = f + 0x9b05688cu;
  g = g + 0x1f83d9abu;
  h = h + 0x5be0cd19u;

  return array<u32,8>(a,b,c,d,e,f,g,h);
}

fn sha256_33(pk: ptr<function, array<u32,9>>) -> array<u32,8> {
  var msg = array<u32,16>();
  for (var i:u32=0u;i<8u;i=i+1u){
    let w = (*pk)[i];
    msg[i] = ((w & 0x000000ffu) << 24) | ((w & 0x0000ff00u) << 8) | ((w & 0x00ff0000u) >> 8) | ((w & 0xff000000u) >> 24);
  }
  let w8 = (*pk)[8u];
  msg[8] = ((w8 & 0x000000ffu) << 24) | (0x80u << 16);
  for (var i:u32=9u;i<15u;i=i+1u){ msg[i]=0u; }
  msg[15] = 264u;
  return sha256_transform(msg);
}

fn rotl(x:u32,n:u32)->u32{ return (x << n) | (x >> (32u-n)); }

fn f_rip(j:u32,x:u32,y:u32,z:u32)->u32{
  if (j<=15u){ return x ^ y ^ z; }
  if (j<=31u){ return (x & y) | (~x & z); }
  if (j<=47u){ return (x | ~y) ^ z; }
  if (j<=63u){ return (x & z) | (y & ~z); }
  return x ^ (y | ~z);
}

fn ripemd160_32(sha: array<u32,8>) -> array<u32,5> {
  var r = array<u32,80>(
    0u,1u,2u,3u,4u,5u,6u,7u,8u,9u,10u,11u,12u,13u,14u,15u,
    7u,4u,13u,1u,10u,6u,15u,3u,12u,0u,9u,5u,2u,14u,11u,8u,
    3u,10u,14u,4u,9u,15u,8u,1u,2u,7u,0u,6u,13u,11u,5u,12u,
    1u,9u,11u,10u,0u,8u,12u,4u,13u,3u,7u,15u,14u,5u,6u,2u,
    4u,0u,5u,9u,7u,12u,2u,10u,14u,1u,3u,8u,11u,6u,15u,13u);
  var rp = array<u32,80>(
    5u,14u,7u,0u,9u,2u,11u,4u,13u,6u,15u,8u,1u,10u,3u,12u,
    6u,11u,3u,7u,0u,13u,5u,10u,14u,15u,8u,12u,4u,9u,1u,2u,
    15u,5u,1u,3u,7u,14u,6u,9u,11u,8u,12u,2u,10u,0u,13u,4u,
    8u,6u,4u,1u,3u,11u,15u,0u,5u,12u,2u,13u,9u,7u,10u,14u,
    12u,15u,10u,4u,1u,5u,8u,7u,6u,2u,13u,14u,0u,3u,9u,11u);
  var s = array<u32,80>(
    11u,14u,15u,12u,5u,8u,7u,9u,11u,13u,14u,15u,6u,7u,9u,8u,
    7u,6u,8u,13u,11u,9u,7u,15u,7u,12u,15u,9u,11u,7u,13u,12u,
    11u,13u,6u,7u,14u,9u,13u,15u,14u,8u,13u,6u,5u,12u,7u,5u,
    11u,12u,14u,15u,14u,15u,9u,8u,9u,14u,5u,6u,8u,6u,5u,12u,
    9u,15u,5u,11u,6u,8u,13u,12u,5u,12u,13u,14u,11u,8u,5u,6u);
  var sp = array<u32,80>(
    8u,9u,9u,11u,13u,15u,15u,5u,7u,7u,8u,11u,14u,14u,12u,6u,
    9u,13u,15u,7u,12u,8u,9u,11u,7u,7u,12u,7u,6u,15u,13u,11u,
    9u,7u,15u,11u,8u,6u,6u,14u,12u,13u,5u,14u,13u,13u,7u,5u,
    15u,5u,8u,11u,14u,14u,6u,14u,6u,9u,12u,9u,12u,5u,15u,8u,
    8u,5u,12u,9u,12u,5u,14u,6u,8u,13u,6u,5u,15u,13u,11u,11u);
  var w = array<u32,16>();
  for (var i:u32=0u;i<8u;i=i+1u){
    let be = sha[i];
    w[i] = ((be & 0x000000ffu) << 24) | ((be & 0x0000ff00u) << 8) | ((be & 0x00ff0000u) >> 8) | ((be & 0xff000000u) >> 24);
  }
  w[8]=0x00000080u;
  for (var i:u32=9u;i<14u;i=i+1u){w[i]=0u;}
  w[14]=256u;
  w[15]=0u;

  var h0:u32=0x67452301u;
  var h1:u32=0xefcdab89u;
  var h2:u32=0x98badcfeu;
  var h3:u32=0x10325476u;
  var h4:u32=0xc3d2e1f0u;

  var a=h0; var b=h1; var c=h2; var d=h3; var e=h4;
  var a2=h0; var b2=h1; var c2=h2; var d2=h3; var e2=h4;

  for (var j:u32=0u;j<80u;j=j+1u){
    let t = rotl(a + f_rip(j,b,c,d) + w[r[j]] +
      (if (j<=15u){0x00000000u}else if(j<=31u){0x5a827999u}else if(j<=47u){0x6ed9eba1u}else if(j<=63u){0x8f1bbcdcu}else{0xa953fd4eu}), s[j]) + e;
    a=e; e=d; d=rotl(c,10u); c=b; b=t;

    let tp = rotl(a2 + f_rip(79u-j,b2,c2,d2) + w[rp[j]] +
      (if (j<=15u){0x50a28be6u}else if(j<=31u){0x5c4dd124u}else if(j<=47u){0x6d703ef3u}else if(j<=63u){0x7a6d76e9u}else{0x00000000u}), sp[j]) + e2;
    a2=e2; e2=d2; d2=rotl(c2,10u); c2=b2; b2=tp;
  }

  let t = h1 + c + d2;
  h1 = h2 + d + e2;
  h2 = h3 + e + a2;
  h3 = h4 + a + b2;
  h4 = h0 + b + c2;
  h0 = t;

  return array<u32,5>(h0,h1,h2,h3,h4);
}

@compute @workgroup_size(256)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
  let idx = gid.x;
  if (idx >= params.n) { return; }
  let base = idx*9u;
  var pk = array<u32,9>();
  for (var i:u32=0u;i<9u;i=i+1u){ pk[i]=inbuf[base+i]; }
  let sha = sha256_33(&pk);
  let rip = ripemd160_32(sha);
  let hit = (rip[0]==params.target0 && rip[1]==params.target1 && rip[2]==params.target2 && rip[3]==params.target3 && rip[4]==params.target4);
  hits[idx] = select(0u,1u,hit);
}
