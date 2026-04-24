// kernel/src/memory/huge_pages/mod.rs
//
// Module huge_pages — THP + hugetlbfs + split PDE.

pub mod hugetlbfs;
pub mod split;
pub mod thp;

pub use thp::{
    alloc_huge_page, free_huge_page, split_huge_page, try_promote_to_huge, ThpConfig, ThpMode,
    ThpStats, HUGE_PAGE_ORDER, THP_CONFIG, THP_STATS,
};

pub use split::{
    generate_split_ptes, split_huge_pde, split_huge_pde_in_place, HugePdeFlags, SplitResult,
    SplitStats, SPLIT_STATS,
};

pub use hugetlbfs::{
    hugetlb_alloc_1gib, hugetlb_alloc_2mib, hugetlb_free_1gib, hugetlb_free_1gib_count,
    hugetlb_free_2mib, hugetlb_free_2mib_count, init as hugetlb_init, HugeTlbSize, HugeTlbStats,
    GIGA_PAGE_ORDER, HUGETLB_POOL, HUGETLB_STATS,
};
