//! This library helps with estimating the progress of a task.

#![deny(missing_docs, rust_2018_idioms, unused, unused_crate_dependencies, unused_import_braces, unused_lifetimes, unused_qualifications, warnings)]
#![forbid(unsafe_code)]

use {
    std::convert::TryFrom,
    async_trait::async_trait,
};
#[cfg(feature = "async-proto")] use async_proto::Protocol;
#[cfg(feature = "serde")] use serde::{
    Deserialize,
    Serialize,
};

mod std_types;

/// A type representing a percentage.
///
/// Guarantees that the value will be between 0 and 100 inclusive.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "async-proto", derive(Protocol))] //TODO check bounds on read
#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))] //TODO check bounds on deserialize
#[cfg_attr(feature = "serde", serde(transparent))]
pub struct Percent(u8);

impl Percent {
    /// 100%.
    pub const MAX: Percent = Percent(100);

    /// # Panics
    ///
    /// If `value` is greater than 100.
    pub fn new(value: u8) -> Percent {
        Percent::try_from(value).expect("percentage above 100")
    }

    /// Returns the percentage representing the fraction `num / denom`, rounded down.
    ///
    /// Truncates the result to fit within the range.
    pub fn fraction(num: usize, denom: usize) -> Percent {
        Percent::try_from(100 * num / denom).unwrap_or(Percent::MAX)
    }
}

macro_rules! percent_conversion {
    ($($T:ty),*) => {
        $(
            impl TryFrom<$T> for Percent {
                type Error = $T;

                #[allow(unused_comparisons)]
                fn try_from(value: $T) -> Result<Percent, $T> {
                    if value >= 0 && value <= 100 {
                        Ok(Percent(value as u8))
                    } else {
                        Err(value)
                    }
                }
            }

            impl From<Percent> for $T {
                fn from(Percent(value): Percent) -> $T { value as $T }
            }
        )*
    };
}

percent_conversion!(u8, u16, u32, u64, u128, usize, i8, i16, i32, i64, i128, isize);

/// A type implementing this trait can estimate the progress of a task.
pub trait Progress {
    /// Returns the estimated progress.
    ///
    /// Should generally round down rather than up. 100 means the task is complete, but 0 doesn't necessarily mean no work has been completed.
    fn progress(&self) -> Percent;
}

/// A task that can run asynchronously and report its progress.
///
/// It also returns a value of type `T` when completed.
#[async_trait]
pub trait Task<T>: Progress + Sized {
    /// Runs the task until the next progress change.
    ///
    /// If this completes the task, the value is returned.
    ///
    /// If it doesn't, the current task is returned, which can be checked using the `Progress` trait, then run again to continue.
    async fn run(self) -> Result<T, Self>;
}
