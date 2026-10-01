//! # `ranking`
//!
//! Deterministic, explainable scoring. There is no machine learning here and
//! there never needs to be: a desktop search that you cannot reason about is a
//! desktop search you cannot trust.
//!
//! The score for a result is the sum of a small number of named components:
//!
//! | component          | default weight | meaning                                    |
//! |--------------------|---------------:|--------------------------------------------|
//! | `exact_match`      |            100 | the name equals the query                  |
//! | `prefix_match`     |             60 | the name starts with the query             |
//! | `filename_match`   |             40 | the query occurs inside the name           |
//! | `token_match`      |             30 | the query equals a whole word of the name  |
//! | `path_match`       |             20 | the query occurs inside the path           |
//! | `fuzzy_match`      |             10 | the query is an ordered subsequence        |
//! | `usage_frequency`  |              8 | you picked this result before             |
//! | `recency`          |              5 | how recently the entity changed           |
//! | `entity_preference`|        per type| applications rank above raw files, etc.    |
//!
//! Every weight lives in [`RankingWeights`], so tuning ranking never means
//! touching the scoring code.

#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod heuristic;
pub mod usage;
pub mod weights;

pub use heuristic::{HeuristicRanker, ScoreBreakdown};
pub use usage::{UsageEntry, UsageIndex};
pub use weights::{EntityPreferences, RankingWeights};
