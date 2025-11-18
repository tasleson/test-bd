use std::fmt;
use std::ops::AddAssign;
use std::ops::Sub;

use serde::Deserialize;
use serde::Serialize;

/// A position index representing an 8-byte aligned offset in a block device.
///
/// `IndexPos` is a type-safe wrapper around a `u64` that represents positions
/// in units of 8 bytes. This is used throughout the library to ensure proper
/// alignment and avoid confusion between byte offsets and index positions.
///
/// # Examples
///
/// ```
/// use test_bd::IndexPos;
///
/// // Create a position at index 100 (which is byte offset 800)
/// let pos = IndexPos::new(100);
/// assert_eq!(pos.as_u64(), 100);
/// assert_eq!(pos.as_abs_byte_offset(), 800);
/// ```
#[derive(Copy, Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[repr(transparent)]
pub struct IndexPos(u64);

impl IndexPos {
    /// Creates a new `IndexPos` from an index value.
    ///
    /// # Arguments
    ///
    /// * `idx` - The index value (in units of 8 bytes)
    ///
    /// # Examples
    ///
    /// ```
    /// use test_bd::IndexPos;
    ///
    /// let pos = IndexPos::new(42);
    /// assert_eq!(pos.as_u64(), 42);
    /// ```
    pub const fn new(idx: u64) -> Self {
        IndexPos(idx)
    }

    /// Returns the raw index value as a `u64`.
    ///
    /// # Examples
    ///
    /// ```
    /// use test_bd::IndexPos;
    ///
    /// let pos = IndexPos::new(100);
    /// assert_eq!(pos.as_u64(), 100);
    /// ```
    pub const fn as_u64(self) -> u64 {
        self.0
    }

    /// Converts the index position to an absolute byte offset.
    ///
    /// Since each index represents 8 bytes, this returns `index * 8`.
    /// However, this could change, so use this method.
    ///
    /// # Examples
    ///
    /// ```
    /// use test_bd::IndexPos;
    ///
    /// let pos = IndexPos::new(100);
    /// assert_eq!(pos.as_abs_byte_offset(), 800);
    /// ```
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
