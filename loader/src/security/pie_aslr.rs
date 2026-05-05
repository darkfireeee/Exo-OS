pub fn deterministic_slide(seed: u64, max_slide_pages: u64) -> u64 {
    if max_slide_pages == 0 {
        return 0;
    }
    let mixed = seed ^ seed.rotate_left(17) ^ 0x9e37_79b9_7f4a_7c15;
    (mixed % max_slide_pages) * 4096
}
