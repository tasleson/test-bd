use std::fmt;
use std::ops::AddAssign;
use std::ops::Sub;

use serde::Deserialize;
use serde::Serialize;

#[derive(Copy, Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[repr(transparent)]
pub struct IndexPos(u64);

impl IndexPos {
    pub const fn new(idx: u64) -> Self {
        IndexPos(idx)
    }

    pub const fn as_u64(self) -> u64 {
        self.0
    }

    pub const fn as_abs_byte_offset(self) -> u64 {
        self.0 * 8
    }
}

impl fmt::Display for IndexPos {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "IndexPos({})", self.0)
    }
}

impl AddAssign for IndexPos {
    fn add_assign(&mut self, other: Self) {
        self.0 += other.0;
    }
}

impl Sub for IndexPos {
    type Output = u64;

    fn sub(self, rhs: IndexPos) -> Self::Output {
        self.0 - rhs.0
    }
}
