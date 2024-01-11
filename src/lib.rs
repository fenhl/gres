//! This library helps with estimating the progress of a task.

#![deny(missing_docs, rust_2018_idioms, unused, unused_crate_dependencies, unused_import_braces, unused_lifetimes, unused_qualifications, warnings)]
#![forbid(unsafe_code)]

use {
    std::{
        convert::TryFrom,
        fmt,
        future::Future,
    },
    async_trait::async_trait,
};
#[cfg(feature = "async-proto")] use async_proto::Protocol;
#[cfg(feature = "serde")] use serde::{
    Serialize,
    de::{
        Deserialize,
        Deserializer,
        Error as _,
        Unexpected,
    },
};

#[cfg(feature = "cli")] pub mod cli;
mod std_types;

/// A type representing a percentage.
///
/// Guarantees that the value will be between 0 and 100 inclusive.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "async-proto", derive(Protocol), async_proto(via = u8, map_err = async_proto::ReadErrorKind::UnknownVariant8))]
#[cfg_attr(feature = "serde", derive(Serialize), serde(transparent))]
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
    ///
    /// `0 / 0` is interpreted as 100%.
    pub fn fraction(num: usize, denom: usize) -> Percent {
        Percent::try_from(num.saturating_mul(100).checked_div(denom).unwrap_or(100)).unwrap_or(Percent::MAX)
    }
}

macro_rules! percent_conversion {
    ($($T:ty),*) => {
        $(
            impl TryFrom<$T> for Percent {
                type Error = $T;

                #[allow(unused_comparisons)]
                fn try_from(value: $T) -> Result<Self, $T> {
                    if value >= 0 && value <= 100 {
                        Ok(Self(value as u8))
                    } else {
                        Err(value)
                    }
                }
            }

            impl<'a> TryFrom<&'a $T> for Percent {
                type Error = $T;

                #[allow(unused_comparisons)]
                fn try_from(&value: &$T) -> Result<Self, $T> {
                    if value >= 0 && value <= 100 {
                        Ok(Self(value as u8))
                    } else {
                        Err(value)
                    }
                }
            }

            impl From<Percent> for $T {
                fn from(Percent(value): Percent) -> Self { value as Self }
            }

            impl<'a> From<&'a Percent> for $T {
                fn from(&Percent(value): &Percent) -> Self { value as Self }
            }
        )*
    };
}

percent_conversion!(u8, u16, u32, u64, u128, usize, i8, i16, i32, i64, i128, isize);

impl From<Percent> for f32 {
    fn from(Percent(value): Percent) -> Self { value.into() }
}

impl From<Percent> for f64 {
    fn from(Percent(value): Percent) -> Self { value.into() }
}

// Deserialize is manually implememted to generate an error for out-of-range values
#[cfg(feature = "serde")]
impl<'de> Deserialize<'de> for Percent {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        u8::deserialize(deserializer)
            .and_then(|value| Self::try_from(value).map_err(|_| D::Error::invalid_value(Unexpected::Unsigned(value.into()), &"value between 0 and 100 (inclusive)")))
    }
}

//TODO check bounds on deserialize

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

/// A [`Task`] that simply runs a given future to completion, without giving intermittent progress updates.
#[derive(Debug)]
pub struct SimpleTask<'a, T: Future + Send> {
    /// The future to run.
    pub fut: T,
    /// Used as the task's [`Display`](fmt::Display) implementation.
    pub label: &'a str,
}

impl<'a, T: Future + Send> fmt::Display for SimpleTask<'a, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.label.fmt(f)
    }
}

impl<'a, T: Future + Send> Progress for SimpleTask<'a, T> {
    fn progress(&self) -> Percent {
        Percent::default()
    }
}

#[async_trait]
impl<'a, T: Future + Send> Task<T::Output> for SimpleTask<'a, T> {
    async fn run(self) -> Result<T::Output, Self> {
        Ok(self.fut.await)
    }
}

/// Convenience function for working with `Task<Result>`.
pub async fn transpose<Task, T, E>(fut: impl Future<Output = Result<Result<T, Task>, E>>) -> Result<Result<T, E>, Task> {
    match fut.await {
        Ok(Ok(x)) => Ok(Ok(x)),
        Ok(Err(step)) => Err(step),
        Err(e) => Ok(Err(e)),
    }
}
