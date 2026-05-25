#[path = "../src/routing.rs"]
mod routing;

use routing::RouteTable;

#[test]
fn lookup_uses_longest_prefix_then_metric() {
    let mut table = RouteTable::new();
    table.add(0, 0, 0x0a00_0202, 100).unwrap();
    table.add(0x0a00_0000, 8, 0, 10).unwrap();
    table.add(0x0a00_0200, 24, 0x0a00_0201, 5).unwrap();

    assert_eq!(table.lookup(0x0a00_020f), Some(0x0a00_0201));
    assert_eq!(table.lookup(0x0a22_3344), Some(0x0a22_3344));
    assert_eq!(table.lookup(0x0808_0808), Some(0x0a00_0202));
    assert_eq!(table.default_gateway(), Some(0x0a00_0202));
}
