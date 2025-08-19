const MAX_HITS : u32 = 1024u;

struct Hits {
  count : atomic<u32>,
  idx   : array<u32, MAX_HITS>,
};

@group(0) @binding(2)
var<storage, read_write> hits : Hits;
