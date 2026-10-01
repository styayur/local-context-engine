//! Ranking weights.
//!
//! Kept in one serialisable struct so a build can be tuned without a code
//! change, and so `--explain` can show the user exactly why something ranked
//! where it did.

use serde::{Deserialize, Serialize};

use search_core::EntityType;

/// How much each scoring component contributes.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct RankingWeights {
    /// The entity name equals the query exactly.
    pub exact_match: f32,
    /// The entity name starts with the query.
    pub prefix_match: f32,
    /// The query occurs inside the entity name.
    pub filename_match: f32,
    /// The query equals a whole word of the entity name.
    pub token_match: f32,
    /// The query occurs inside the entity path.
    pub path_match: f32,
    /// The query is an ordered subsequence of the entity name.
    pub fuzzy_match: f32,
    /// The user selected this entity before.
    pub usage_frequency: f32,
    /// How recently the entity changed.
    pub recency: f32,
    /// Per-type preference multiplier.
    pub entity_preference: EntityPreferences,
    /// Scores below this never produce a result.
    pub minimum_score: f32,
    /// A path-only match is discounted by this factor.
    pub path_only_factor: f32,
}

/// Per-entity-type preference. Higher means "show me this first".
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct EntityPreferences {
    /// Preference applied to files.
    pub file: f32,
    /// Preference applied to directories.
    pub directory: f32,
    /// Preference applied to processes.
    pub process: f32,
    /// Preference applied to applications.
    pub application: f32,
    /// Preference applied to services.
    pub service: f32,
    /// Preference applied to windows.
    pub window: f32,
}

impl Default for RankingWeights {
    fn default() -> Self {
        Self {
            exact_match: 100.0,
            prefix_match: 60.0,
            filename_match: 40.0,
            token_match: 30.0,
            path_match: 20.0,
            fuzzy_match: 10.0,
            usage_frequency: 8.0,
            recency: 5.0,
            entity_preference: EntityPreferences::default(),
            minimum_score: 1.0,
            path_only_factor: 0.5,
        }
    }
}

impl Default for EntityPreferences {
    fn default() -> Self {
        Self {
            file: 4.0,
            directory: 3.0,
            process: 5.0,
            application: 8.0,
            service: 2.0,
            window: 2.0,
        }
    }
}

impl EntityPreferences {
    /// Preference for one entity type.
    #[must_use]
    pub const fn for_type(&self, entity_type: EntityType) -> f32 {
        match entity_type {
            EntityType::File => self.file,
            EntityType::Directory => self.directory,
            EntityType::Process => self.process,
            EntityType::Application => self.application,
            EntityType::Service => self.service,
            EntityType::Window => self.window,
        }
    }
}

impl RankingWeights {
    /// The documented defaults, spelled out so tests can assert them.
    #[must_use]
    pub fn documented() -> Self {
        Self::default()
    }

    /// Scale every component by `factor`. Useful for building alternative
    /// profiles (for example "prefer files") without hand-tuning.
    #[must_use]
    pub fn scaled(mut self, factor: f32) -> Self {
        self.exact_match *= factor;
        self.prefix_match *= factor;
        self.filename_match *= factor;
        self.token_match *= factor;
        self.path_match *= factor;
        self.fuzzy_match *= factor;
        self.usage_frequency *= factor;
        self.recency *= factor;
        self.entity_preference.file *= factor;
        self.entity_preference.directory *= factor;
        self.entity_preference.process *= factor;
        self.entity_preference.application *= factor;
        self.entity_preference.service *= factor;
        self.entity_preference.window *= factor;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_documented_values() {
        let weights = RankingWeights::documented();
        assert!((weights.exact_match - 100.0).abs() < f32::EPSILON);
        assert!((weights.prefix_match - 60.0).abs() < f32::EPSILON);
        assert!((weights.filename_match - 40.0).abs() < f32::EPSILON);
        assert!((weights.token_match - 30.0).abs() < f32::EPSILON);
        assert!((weights.path_match - 20.0).abs() < f32::EPSILON);
        assert!((weights.fuzzy_match - 10.0).abs() < f32::EPSILON);
        assert!((weights.usage_frequency - 8.0).abs() < f32::EPSILON);
        assert!((weights.recency - 5.0).abs() < f32::EPSILON);
    }

    #[test]
    fn applications_are_preferred_over_files() {
        let preferences = EntityPreferences::default();
        assert!(
            preferences.for_type(EntityType::Application) > preferences.for_type(EntityType::File)
        );
        assert!(
            preferences.for_type(EntityType::Process) > preferences.for_type(EntityType::Service)
        );
    }

    #[test]
    fn weights_round_trip_through_json() {
        let weights = RankingWeights::default();
        let json = serde_json::to_string(&weights).unwrap();
        let parsed: RankingWeights = serde_json::from_str(&json).unwrap();
        assert_eq!(weights, parsed);
    }
}
