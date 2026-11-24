//! A player's character, the chemistry between two team-mates on the pitch, and how well a side
//! knows its shape. Data and value semantics only: what character or chemistry does to a match,
//! to training or to a contract is decided in `ofm_core` (`character`, `chemistry`).
//!
//! Pair chemistry is an on-pitch understanding built from minutes played together and goals
//! made together. It is never a statement about two real people's personal relationship.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// The lowest and highest a character trait reads.
pub const TRAIT_MIN: u8 = 1;
pub const TRAIT_MAX: u8 = 99;
/// What every trait reads for a player nobody has assessed: the middle of the scale.
pub const TRAIT_NEUTRAL: u8 = 50;

/// Eight traits of a player's character, each `1..=99`; 50 is the average player.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(default)]
pub struct CharacterProfile {
    pub professionalism: u8,
    pub ambition: u8,
    pub loyalty: u8,
    pub consistency: u8,
    pub big_matches: u8,
    pub pressure_handling: u8,
    pub injury_proneness: u8,
    pub adaptability: u8,
}

impl Default for CharacterProfile {
    fn default() -> Self {
        Self {
            professionalism: TRAIT_NEUTRAL,
            ambition: TRAIT_NEUTRAL,
            loyalty: TRAIT_NEUTRAL,
            consistency: TRAIT_NEUTRAL,
            big_matches: TRAIT_NEUTRAL,
            pressure_handling: TRAIT_NEUTRAL,
            injury_proneness: TRAIT_NEUTRAL,
            adaptability: TRAIT_NEUTRAL,
        }
    }
}

impl CharacterProfile {
    /// The same profile with every trait held to `1..=99`, for a value read from a save or a
    /// package that may hold anything a `u8` can.
    pub fn clamped(self) -> Self {
        let hold = |value: u8| value.clamp(TRAIT_MIN, TRAIT_MAX);
        Self {
            professionalism: hold(self.professionalism),
            ambition: hold(self.ambition),
            loyalty: hold(self.loyalty),
            consistency: hold(self.consistency),
            big_matches: hold(self.big_matches),
            pressure_handling: hold(self.pressure_handling),
            injury_proneness: hold(self.injury_proneness),
            adaptability: hold(self.adaptability),
        }
    }

    /// Whether every trait still reads the neutral default: a player nobody has assessed.
    pub fn is_unassessed(&self) -> bool {
        *self == Self::default()
    }
}

/// A one-word summary of a character, for a scout report or a squad list. A profile may carry
/// several; `Balanced` only when it carries no other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
pub enum CharacterLabel {
    ModelProfessional,
    Driven,
    Loyal,
    BigGamePlayer,
    Consistent,
    Temperamental,
    InjuryProne,
    Balanced,
}

/// The highest and lowest a pair's bond reads.
pub const BOND_MIN: i8 = -100;
pub const BOND_MAX: i8 = 100;

/// How well two team-mates understand each other on the pitch: the minutes they have shared and
/// a bond built from them and from goals made together. Stored with the two ids in canonical
/// order (`player_a <= player_b`), so a pair has exactly one entry whichever way it is named.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairChemistry {
    pub player_a: String,
    pub player_b: String,
    #[serde(default)]
    pub shared_minutes: u32,
    #[serde(default)]
    pub bond: i8,
}

impl PairChemistry {
    /// The pair of `first` and `second`, in canonical order, with `bond` held to `-100..=100`.
    pub fn new(first: &str, second: &str, shared_minutes: u32, bond: i8) -> Self {
        let (player_a, player_b) = Self::canonical(first, second);
        Self {
            player_a: player_a.to_string(),
            player_b: player_b.to_string(),
            shared_minutes,
            bond: bond.clamp(BOND_MIN, BOND_MAX),
        }
    }

    /// The two ids in the order a pair is stored in.
    pub fn canonical<'a>(first: &'a str, second: &'a str) -> (&'a str, &'a str) {
        if first <= second {
            (first, second)
        } else {
            (second, first)
        }
    }

    /// Whether this is the pair of `first` and `second`, named in either order.
    pub fn is_pair(&self, first: &str, second: &str) -> bool {
        let (a, b) = Self::canonical(first, second);
        self.player_a == a && self.player_b == b
    }

    /// Whether `player_id` is one of the two.
    pub fn involves(&self, player_id: &str) -> bool {
        self.player_a == player_id || self.player_b == player_id
    }

    /// The bond moved by `delta`, held to `-100..=100`.
    pub fn adjust_bond(&mut self, delta: i16) {
        let moved = (i16::from(self.bond) + delta).clamp(i16::from(BOND_MIN), i16::from(BOND_MAX));
        self.bond = i8::try_from(moved).unwrap_or(if moved < 0 { BOND_MIN } else { BOND_MAX });
    }
}

/// The highest a side's familiarity with its shape reads.
pub const FAMILIARITY_MAX: f32 = 100.0;

/// How well a side knows the formation it plays: 0 for a shape it has never played, 100 for one
/// it plays in its sleep.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(default)]
pub struct TeamTacticalFamiliarity {
    /// The formation the familiarity was built in; empty before the side's first match.
    pub formation: String,
    pub familiarity: f32,
    pub matches_together: u32,
}

impl Default for TeamTacticalFamiliarity {
    fn default() -> Self {
        Self {
            formation: String::new(),
            familiarity: 0.0,
            matches_together: 0,
        }
    }
}

impl TeamTacticalFamiliarity {
    /// A familiarity with `familiarity` held to `0..=100` (a NaN reads as 0).
    pub fn new(formation: impl Into<String>, familiarity: f32, matches_together: u32) -> Self {
        Self {
            formation: formation.into(),
            familiarity: Self::hold(familiarity),
            matches_together,
        }
    }

    /// `value` held to `0..=100`; NaN reads as 0.
    pub fn hold(value: f32) -> f32 {
        if value.is_nan() {
            0.0
        } else {
            value.clamp(0.0, FAMILIARITY_MAX)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Given a player nobody has assessed
    /// When his character is read
    /// Then every trait is the neutral 50
    #[test]
    fn an_unassessed_character_reads_fifty_in_every_trait() {
        let profile = CharacterProfile::default();
        assert!(profile.is_unassessed());
        assert_eq!(profile.professionalism, 50);
        assert_eq!(profile.adaptability, 50);
    }

    /// Given a profile with traits at 0 and 255
    /// When it is clamped
    /// Then they read 1 and 99
    #[test]
    fn a_profile_out_of_range_is_held_to_one_to_ninety_nine() {
        let wild = CharacterProfile {
            professionalism: 0,
            ambition: 255,
            ..CharacterProfile::default()
        }
        .clamped();
        assert_eq!((wild.professionalism, wild.ambition), (1, 99));
    }

    /// Given a save that predates character and holds only some traits
    /// When it is read
    /// Then the missing traits read as neutral
    #[test]
    fn a_partial_profile_reads_missing_traits_as_neutral() {
        let profile: CharacterProfile = serde_json::from_str(r#"{"ambition":80}"#).unwrap();
        assert_eq!(profile.ambition, 80);
        assert_eq!(profile.loyalty, 50);
    }

    /// Given a pair named in either order
    /// When it is made
    /// Then it is stored in canonical order and recognised both ways
    #[test]
    fn a_pair_is_stored_in_canonical_order() {
        let pair = PairChemistry::new("zed", "amy", 90, 5);
        assert_eq!(
            (pair.player_a.as_str(), pair.player_b.as_str()),
            ("amy", "zed")
        );
        assert!(pair.is_pair("zed", "amy") && pair.is_pair("amy", "zed"));
        assert!(pair.involves("zed") && !pair.involves("bob"));
    }

    /// Given a bond near either end
    /// When it is moved past it
    /// Then it stops at -100 or 100
    #[test]
    fn a_bond_is_held_to_minus_one_hundred_to_one_hundred() {
        let mut pair = PairChemistry::new("a", "b", 0, 95);
        pair.adjust_bond(40);
        assert_eq!(pair.bond, 100);
        pair.adjust_bond(-300);
        assert_eq!(pair.bond, -100);
        assert_eq!(PairChemistry::new("a", "b", 0, i8::MIN).bond, -100);
    }

    /// Given familiarity readings below, above and not a number
    /// When a familiarity is made from each
    /// Then they read 0, 100 and 0
    #[test]
    fn familiarity_is_held_to_zero_to_one_hundred() {
        assert_eq!(
            TeamTacticalFamiliarity::new("4-4-2", -5.0, 0).familiarity,
            0.0
        );
        assert_eq!(
            TeamTacticalFamiliarity::new("4-4-2", 150.0, 0).familiarity,
            100.0
        );
        assert_eq!(
            TeamTacticalFamiliarity::new("4-4-2", f32::NAN, 0).familiarity,
            0.0
        );
    }
}
