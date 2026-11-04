//! Resource accounting for fixtures.
//!
//! A single fixed `{cpu_mili, memory_bytes}` [`Capacity`]. Generalizing this
//! to a named-resource
//! budget set keyed by `(profile_id, pool_id)` is a later step; the admission
//! and arithmetic shape here port directly to it.

use std::cmp::max;
use std::fmt::{self, Display, Formatter};

use serde::{Deserialize, Serialize};

/// Quantities of resources needed to run a fixture.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize, Serialize)]
pub struct Capacity {
    pub cpu_mili: i64,
    pub memory_bytes: i64,
}

impl Display for Capacity {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}mCPU, {}MB mem",
            self.cpu_mili,
            self.memory_bytes / 1024 / 1024
        )
    }
}

impl Capacity {
    pub fn zero() -> Self {
        Capacity {
            cpu_mili: 0,
            memory_bytes: 0,
        }
    }

    /// Whether any one component of `self` is greater than that component of `other`.
    pub fn any_greater(&self, other: &Self) -> bool {
        self.memory_bytes > other.memory_bytes || self.cpu_mili > other.cpu_mili
    }

    pub fn is_zero(&self) -> bool {
        self.memory_bytes == 0 && self.cpu_mili == 0
    }
}

impl std::ops::Sub for &Capacity {
    type Output = Capacity;
    fn sub(self, rhs: Self) -> Self::Output {
        Capacity {
            cpu_mili: self.cpu_mili - rhs.cpu_mili,
            memory_bytes: self.memory_bytes - rhs.memory_bytes,
        }
    }
}

impl std::ops::AddAssign<&Capacity> for Capacity {
    fn add_assign(&mut self, rhs: &Self) {
        self.cpu_mili += rhs.cpu_mili;
        self.memory_bytes += rhs.memory_bytes;
    }
}

impl std::ops::Add for &Capacity {
    type Output = Capacity;
    fn add(self, rhs: Self) -> Self::Output {
        Capacity {
            cpu_mili: self.cpu_mili + rhs.cpu_mili,
            memory_bytes: self.memory_bytes + rhs.memory_bytes,
        }
    }
}

impl std::ops::Mul<i64> for &Capacity {
    type Output = Capacity;
    fn mul(self, rhs: i64) -> Self::Output {
        Capacity {
            cpu_mili: self.cpu_mili * rhs,
            memory_bytes: self.memory_bytes * rhs,
        }
    }
}

impl std::ops::Div<i64> for &Capacity {
    type Output = Capacity;
    fn div(self, rhs: i64) -> Self::Output {
        Capacity {
            cpu_mili: self.cpu_mili / rhs,
            memory_bytes: self.memory_bytes / rhs,
        }
    }
}

/// Total available resources and current remaining headroom.
#[derive(Debug, Clone)]
pub struct Quota {
    pub total: Capacity,
    pub remaining: Capacity,
}

impl Quota {
    /// Build a quota from a total and the amount already consumed.
    pub fn new(total: Capacity, consumed: &Capacity) -> Result<Self, QuotaError> {
        let mut quota = Quota {
            remaining: total.clone(),
            total,
        };
        quota.try_consume(consumed)?;
        Ok(quota)
    }

    /// Increase usage by `allocation`, or `Err` if it would exceed the quota.
    /// On error the quota is unchanged.
    pub fn try_consume(&mut self, allocation: &Capacity) -> Result<(), QuotaError> {
        if allocation.any_greater(&self.remaining) {
            Err(QuotaError::Exceeded {
                by: Capacity {
                    cpu_mili: max(0, allocation.cpu_mili - self.remaining.cpu_mili),
                    memory_bytes: max(0, allocation.memory_bytes - self.remaining.memory_bytes),
                },
            })
        } else {
            self.remaining = &self.remaining - allocation;
            Ok(())
        }
    }
}

/// Admission failure: the allocation would exceed the quota by `by`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuotaError {
    Exceeded { by: Capacity },
}

impl Display for QuotaError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            QuotaError::Exceeded { by } => write!(f, "exceeded quota by {by}"),
        }
    }
}

impl std::error::Error for QuotaError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn try_consume_tracks_and_rejects() {
        let total = Capacity {
            cpu_mili: 1000,
            memory_bytes: 64000,
        };
        let mut quota = Quota::new(total.clone(), &Capacity::zero()).unwrap();

        quota
            .try_consume(&Capacity {
                cpu_mili: 400,
                memory_bytes: 30000,
            })
            .unwrap();
        assert_eq!(
            quota.remaining,
            Capacity {
                cpu_mili: 600,
                memory_bytes: 34000
            }
        );

        // Too much memory: rejected, remaining unchanged, reports the overflow.
        let remaining = quota.remaining.clone();
        let err = quota
            .try_consume(&Capacity {
                cpu_mili: 300,
                memory_bytes: 40000,
            })
            .unwrap_err();
        assert_eq!(
            err,
            QuotaError::Exceeded {
                by: Capacity {
                    cpu_mili: 0,
                    memory_bytes: 6000
                }
            }
        );
        assert_eq!(quota.remaining, remaining);
    }

    #[test]
    fn new_rejects_when_already_over() {
        let total = Capacity {
            cpu_mili: 100,
            memory_bytes: 100,
        };
        let consumed = Capacity {
            cpu_mili: 200,
            memory_bytes: 0,
        };
        assert!(Quota::new(total, &consumed).is_err());
    }
}
