//! TEMPORARY A/B switches for measuring AI changes one seat against three (removed before
//! committing). Every switch is on unless a runner turns it off for some seats.
use std::cell::Cell;

thread_local! {
    /// Switches turned off, per seat.
    static OFF: Cell<[u64; 8]> = Cell::new([parse(&std::env::var("EXP_OFF").unwrap_or_default()); 8]);
}

pub const NAMES: &[&str] = &[
    "twin", "arrows", "net", "tax", "ronin", "decoy", "boom", "plans", "saga", "draw", "edict",
    "ramos", "barrage", "soft", "blink", "colors", "walker", "removal", "explosive", "teamwork",
    "land", "maypay", "trick", "x13", "rmhold", "rmcmd", "rmtarget", "trickhold", "rmscale", "rmscale2", "rmnohold", "rmnever", "rmflat", "rmdanger", "rmscale3", "rmconvex", "rmbomb",
];

pub fn bit(name: &str) -> u64 {
    1u64 << NAMES.iter().position(|n| *n == name).expect("switch")
}

pub fn parse(s: &str) -> u64 {
    if s == "all" {
        return u64::MAX;
    }
    s.split(',').filter(|x| !x.is_empty()).map(bit).fold(0, |a, b| a | b)
}

/// The switches turned off for each seat.
pub fn set_off(off: [u64; 8]) {
    OFF.with(|c| c.set(off))
}

#[inline]
pub fn on(flag: u64, p: usize) -> bool {
    p >= 8 || OFF.with(|c| c.get()[p] & flag == 0)
}

pub const TWIN: u64 = 1 << 0;
pub const ARROWS: u64 = 1 << 1;
pub const NET: u64 = 1 << 2;
pub const TAX: u64 = 1 << 3;
pub const RONIN: u64 = 1 << 4;
pub const DECOY: u64 = 1 << 5;
pub const BOOM: u64 = 1 << 6;
pub const PLANS: u64 = 1 << 7;
pub const SAGA: u64 = 1 << 8;
pub const DRAW: u64 = 1 << 9;
pub const EDICT: u64 = 1 << 10;
pub const RAMOS: u64 = 1 << 11;
pub const BARRAGE: u64 = 1 << 12;
pub const SOFT: u64 = 1 << 13;
pub const BLINK: u64 = 1 << 14;
pub const COLORS: u64 = 1 << 15;
pub const WALKER: u64 = 1 << 16;
pub const REMOVAL: u64 = 1 << 17;
pub const EXPLOSIVE: u64 = 1 << 18;
pub const TEAMWORK: u64 = 1 << 19;
pub const LAND: u64 = 1 << 20;
pub const MAYPAY: u64 = 1 << 21;
pub const TRICK: u64 = 1 << 22;
pub const X13: u64 = 1 << 23;
pub const RMHOLD: u64 = 1 << 24;
pub const RMCMD: u64 = 1 << 25;
pub const RMTARGET: u64 = 1 << 26;
pub const TRICKHOLD: u64 = 1 << 27;
pub const RMSCALE: u64 = 1 << 28;
pub const RMSCALE2: u64 = 1 << 29;
pub const RMNOHOLD: u64 = 1 << 30;
pub const RMNEVER: u64 = 1 << 31;
pub const RMFLAT: u64 = 1 << 32;
pub const RMDANGER: u64 = 1 << 33;
pub const RMSCALE3: u64 = 1 << 34;
pub const RMCONVEX: u64 = 1 << 35;
pub const RMBOMB: u64 = 1 << 36;
