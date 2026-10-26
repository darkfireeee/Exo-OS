use crate::bare::types::*;
use crate::bare::util::*;

pub fn cmd_calendar(_args: &Args) -> i32 {
    write_all(STDOUT, b"      October 2026      \n");
    write_all(STDOUT, b"Su  Mo  Tu  We  Th  Fr  Sa\n");
    write_all(STDOUT, b"                 1   2   3\n");
    write_all(STDOUT, b" 4   5   6   7   8   9  10\n");
    write_all(STDOUT, b"11  12  13  14  15  16  17\n");
    write_all(STDOUT, b"18  19  20  21  22  23  24\n");
    write_all(STDOUT, b"25  26  27  28  29  30  31\n");
    0
}
