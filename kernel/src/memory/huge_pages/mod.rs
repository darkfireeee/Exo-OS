// kernel/src/memory/huge_pages/mod.rs
//
// Module huge_pages — THP + hugetlbfs + split PDE.

pub mod thp;
pub mod split;
pub mod hugetlbfs;

pub use thp::{
    ThpMode, ThpConfig, THP_CONFIG, ThpStats, THP_STATS,
    HUGE_PAGE_ORDER, alloc_huge_page, free_huge_page, split_huge_page, try_promote_to_huge,
};

pub use split::{
    SplitStats, SPLIT_STATS, SplitResult, HugePdeFlags,
    generate_split_ptes, split_huge_pde, split_huge_pde_in_place,
};

pub use hugetlbfs::{
    GIGA_PAGE_ORDER, HugeTlbSize, HugeTlbStats, HUGETLB_STATS, HUGETLB_POOL,
    hugetlb_alloc_2mib, hugetlb_alloc_1gib,
    hugetlb_free_2mib, hugetlb_free_1gib,
    hugetlb_free_2mib_count, hugetlb_free_1gib_count,
    init as hugetlb_init,
};
