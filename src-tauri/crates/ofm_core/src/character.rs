//! What a player's character does: how it is first assessed, the one-word labels a scout puts on
//! it, and the small, bounded factors it puts on training, on a match, on injury risk and on a
//! contract talk. The profile itself is data (`domain::character`).
//!
//! Every factor reads 1.0 for the neutral profile (every trait 50), so a player nobody has
//! assessed plays, trains and negotiates exactly as he did before character existed.

use domain::character::{CharacterLabel, CharacterProfile, TRAIT_MAX, TRAIT_MIN, TRAIT_NEUTRAL};
use domain::player::Player;

use crate::stable_hash::stable_hash;

/// Keeps the character stream from being any other stream on the same player id.
const CHARACTER_SALT: u64 = 0xc4a7_0000_c4a7_0001;

/// A trait at or above this earns its label.
const LABEL_HIGH: u8 = 80;
/// Composure under pressure, or consistency, at or below this reads as temperamental.
const LABEL_LOW: u8 = 25;
/// Injury proneness at or above this reads as injury-prone.
const INJURY_PRONE_FROM: u8 = 75;

/// The character a player shows when first assessed: half what his football already says about
/// him (a composed player handles pressure, a team player is professional) and half his own,
/// drawn from his id and the world's seed. The same player in the same world always reads the
/// same; ambition and loyalty say nothing about his football and are his own alone.
pub fn derive_character_profile(player: &Player, seed: u64) -> CharacterProfile {
    let mut word = stable_hash(player.id.as_bytes(), seed ^ CHARACTER_SALT);
    let mut own = || {
        word = stable_hash(&word.to_le_bytes(), CHARACTER_SALT);
        trait_from_draw(word)
    };
    let a = &player.attributes;
    let average = |x: u8, y: u8| mean_of(&[x, y]);
    let durability = TRAIT_MAX.saturating_sub(average(a.stamina, a.strength));
    CharacterProfile {
        professionalism: lean(average(a.teamwork, a.decisions), own()),
        ambition: own(),
        loyalty: own(),
        consistency: lean(average(a.composure, a.decisions), own()),
        big_matches: lean(a.composure, own()),
        pressure_handling: lean(average(a.composure, a.leadership), own()),
        injury_proneness: lean(durability, own()),
        adaptability: lean(average(a.teamwork, a.vision), own()),
    }
    .clamped()
}

/// The player's character, assessed now if nobody has yet: what the match, training and
/// contract rules read.
pub fn assessed_character(player: &Player, seed: u64) -> CharacterProfile {
    if player.character.is_unassessed() {
        derive_character_profile(player, seed)
    } else {
        player.character
    }
}

/// Give every player nobody has assessed the character he shows when first assessed. Returns how
/// many it assessed, so a caller can tell whether anything changed.
pub fn assess_unassessed_characters(players: &mut [Player], seed: u64) -> usize {
    let mut assessed = 0;
    for player in players.iter_mut().filter(|p| p.character.is_unassessed()) {
        player.character = derive_character_profile(player, seed);
        assessed += 1;
    }
    assessed
}

/// The labels a profile earns. `Balanced` only when it earns no other.
pub fn character_labels(profile: &CharacterProfile) -> Vec<CharacterLabel> {
    let checks = [
        (
            profile.professionalism >= LABEL_HIGH,
            CharacterLabel::ModelProfessional,
        ),
        (profile.ambition >= LABEL_HIGH, CharacterLabel::Driven),
        (profile.loyalty >= LABEL_HIGH, CharacterLabel::Loyal),
        (
            profile.big_matches >= LABEL_HIGH,
            CharacterLabel::BigGamePlayer,
        ),
        (
            profile.consistency >= LABEL_HIGH,
            CharacterLabel::Consistent,
        ),
        (
            profile.pressure_handling <= LABEL_LOW || profile.consistency <= LABEL_LOW,
            CharacterLabel::Temperamental,
        ),
        (
            profile.injury_proneness >= INJURY_PRONE_FROM,
            CharacterLabel::InjuryProne,
        ),
    ];
    let labels: Vec<_> = checks
        .into_iter()
        .filter_map(|(earned, label)| earned.then_some(label))
        .collect();
    if labels.is_empty() {
        vec![CharacterLabel::Balanced]
    } else {
        labels
    }
}

/// The factor on a player's training gains: 0.85 for the least professional and ambitious, 1.18
/// for the most, 1.0 for the neutral profile. Professionalism weighs more than ambition.
pub fn training_character_factor(profile: &CharacterProfile) -> f64 {
    let drive = 0.6 * f64::from(profile.professionalism) + 0.4 * f64::from(profile.ambition);
    around_neutral(drive, 0.85, 1.18)
}

/// The share of his form a player brings to a match: 0.96..=1.04, 1.0 for an ordinary match or
/// the neutral profile. A big match asks for his big-match temperament, a side under pressure for
/// his composure under it; both at once are weighed together.
pub fn character_match_factor(player: &Player, is_big_match: bool, is_under_pressure: bool) -> f64 {
    profile_match_factor(&player.character, is_big_match, is_under_pressure)
}

/// [`character_match_factor`] for a profile in hand, such as one assessed for the match
/// (`assessed_character`).
pub fn profile_match_factor(
    profile: &CharacterProfile,
    is_big_match: bool,
    is_under_pressure: bool,
) -> f64 {
    let mut lift = 0.0;
    if is_big_match {
        lift += around_neutral(f64::from(profile.big_matches), 0.97, 1.03) - 1.0;
    }
    if is_under_pressure {
        lift += around_neutral(f64::from(profile.pressure_handling), 0.98, 1.02) - 1.0;
    }
    (1.0 + lift).clamp(0.96, 1.04)
}

/// The factor on a player's injury risk: 0.8 for the most robust, 1.3 for the most injury-prone,
/// 1.0 for the neutral profile.
pub fn injury_risk_character_factor(profile: &CharacterProfile) -> f64 {
    around_neutral(f64::from(profile.injury_proneness), 0.8, 1.3)
}

/// How readily a player stays: the factor on his willingness to extend a contract, 0.85 for the
/// least loyal and 1.15 for the most, 1.0 for the neutral profile.
pub fn contract_loyalty_factor(profile: &CharacterProfile) -> f64 {
    around_neutral(f64::from(profile.loyalty), 0.85, 1.15)
}

/// `value` (a trait reading, `1..=99`) read onto `low..=high`, with the neutral 50 on 1.0: the
/// two halves of the scale are each linear, so the neutral profile is exactly 1.0 even where the
/// range is lopsided.
fn around_neutral(value: f64, low: f64, high: f64) -> f64 {
    let neutral = f64::from(TRAIT_NEUTRAL);
    let value = value.clamp(f64::from(TRAIT_MIN), f64::from(TRAIT_MAX));
    if value >= neutral {
        1.0 + (high - 1.0) * (value - neutral) / (f64::from(TRAIT_MAX) - neutral)
    } else {
        1.0 - (1.0 - low) * (neutral - value) / (neutral - f64::from(TRAIT_MIN))
    }
}

/// A trait reading from a draw, spread evenly over `1..=99`.
fn trait_from_draw(draw: u64) -> u8 {
    let span = u64::from(TRAIT_MAX - TRAIT_MIN) + 1;
    u8::try_from(draw % span).map_or(TRAIT_NEUTRAL, |offset| TRAIT_MIN + offset)
}

/// Halfway between what the football says and the player's own draw.
fn lean(football: u8, own: u8) -> u8 {
    mean_of(&[football, own])
}

fn mean_of(values: &[u8]) -> u8 {
    let total: u32 = values.iter().map(|value| u32::from(*value)).sum();
    let count = u32::try_from(values.len()).unwrap_or(1).max(1);
    u8::try_from(total / count).unwrap_or(TRAIT_NEUTRAL)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::uniform_attributes;
    use domain::player::Position;

    fn player(id: &str, attribute: u8) -> Player {
        Player::new(
            id.into(),
            "P".into(),
            "Player".into(),
            "1995-01-01".into(),
            "ENG".into(),
            Position::CentralMidfielder,
            uniform_attributes(attribute),
        )
    }

    fn profile_with(change: impl FnOnce(&mut CharacterProfile)) -> CharacterProfile {
        let mut profile = CharacterProfile::default();
        change(&mut profile);
        profile
    }

    /// Given one player assessed twice in one world, and in another world
    /// When his character is derived
    /// Then it is the same both times in one world, and a different world can read differently
    #[test]
    fn a_character_is_the_same_every_time_it_is_assessed_in_one_world() {
        let midfielder = player("p-1", 60);
        assert_eq!(
            derive_character_profile(&midfielder, 7),
            derive_character_profile(&midfielder, 7)
        );
        let worlds: Vec<_> = (0..8)
            .map(|seed| derive_character_profile(&midfielder, seed))
            .collect();
        assert!(worlds.iter().any(|profile| *profile != worlds[0]));
    }

    /// Given many players, composed or not
    /// When their characters are derived
    /// Then every trait is within 1..=99, and the composed ones handle pressure better on average
    #[test]
    fn a_derived_character_is_in_range_and_leans_on_the_football() {
        let mean_pressure = |attribute: u8| {
            let total: u32 = (0..200)
                .map(|i| {
                    let profile = derive_character_profile(&player(&format!("p{i}"), attribute), 3);
                    for value in [
                        profile.professionalism,
                        profile.ambition,
                        profile.loyalty,
                        profile.consistency,
                        profile.big_matches,
                        profile.pressure_handling,
                        profile.injury_proneness,
                        profile.adaptability,
                    ] {
                        assert!((TRAIT_MIN..=TRAIT_MAX).contains(&value));
                    }
                    u32::from(profile.pressure_handling)
                })
                .sum();
            total / 200
        };
        assert!(mean_pressure(90) > mean_pressure(30) + 20);
    }

    /// Given a player nobody has assessed and one already assessed
    /// When the unassessed are assessed
    /// Then only the first changes, and he reads what derivation gives
    #[test]
    fn only_an_unassessed_player_is_assessed() {
        let mut assessed = player("p-a", 60);
        assessed.character = profile_with(|p| p.loyalty = 90);
        let mut players = vec![player("p-u", 60), assessed.clone()];

        assert_eq!(assess_unassessed_characters(&mut players, 11), 1);

        assert_eq!(
            players[0].character,
            derive_character_profile(&players[0], 11)
        );
        assert_eq!(players[1].character, assessed.character);
        assert_eq!(assessed_character(&assessed, 11), assessed.character);
    }

    /// Given a neutral profile, a professional, ambitious, loyal one, and a fragile, temperamental
    /// one
    /// When their labels are read
    /// Then the neutral one is Balanced alone, and the others earn exactly their labels
    #[test]
    fn a_profile_earns_the_labels_of_its_extremes_and_balanced_only_without_any() {
        assert_eq!(
            character_labels(&CharacterProfile::default()),
            vec![CharacterLabel::Balanced]
        );
        let leader = profile_with(|p| {
            p.professionalism = 85;
            p.ambition = 90;
            p.loyalty = 80;
        });
        assert_eq!(
            character_labels(&leader),
            vec![
                CharacterLabel::ModelProfessional,
                CharacterLabel::Driven,
                CharacterLabel::Loyal
            ]
        );
        let fragile = profile_with(|p| {
            p.pressure_handling = 20;
            p.injury_proneness = 80;
        });
        assert_eq!(
            character_labels(&fragile),
            vec![CharacterLabel::Temperamental, CharacterLabel::InjuryProne]
        );
    }

    /// Given the neutral profile and the two extremes
    /// When the training, injury and loyalty factors are read
    /// Then neutral is 1.0 and the extremes are the ends of each range
    #[test]
    fn every_factor_is_one_for_the_neutral_profile_and_bounded_at_the_extremes() {
        let neutral = CharacterProfile::default();
        let low = CharacterProfile {
            professionalism: 1,
            ambition: 1,
            loyalty: 1,
            injury_proneness: 1,
            ..neutral
        };
        let high = CharacterProfile {
            professionalism: 99,
            ambition: 99,
            loyalty: 99,
            injury_proneness: 99,
            ..neutral
        };
        let close = |a: f64, b: f64| (a - b).abs() < 1e-9;
        assert!(close(training_character_factor(&neutral), 1.0));
        assert!(close(training_character_factor(&low), 0.85));
        assert!(close(training_character_factor(&high), 1.18));
        assert!(close(injury_risk_character_factor(&neutral), 1.0));
        assert!(close(injury_risk_character_factor(&low), 0.8));
        assert!(close(injury_risk_character_factor(&high), 1.3));
        assert!(close(contract_loyalty_factor(&neutral), 1.0));
        assert!(close(contract_loyalty_factor(&low), 0.85));
        assert!(close(contract_loyalty_factor(&high), 1.15));
    }

    /// Given a big-game player who handles pressure, one who does neither, and a neutral one
    /// When each plays an ordinary match, a big match, and a big match under pressure
    /// Then an ordinary match is 1.0 for all, the occasion moves the first two apart, and every
    ///      factor stays within 0.96..=1.04
    #[test]
    fn the_match_factor_moves_only_on_an_occasion_and_stays_within_four_percent() {
        let mut big = player("big", 60);
        big.character = profile_with(|p| {
            p.big_matches = 99;
            p.pressure_handling = 99;
        });
        let mut shy = player("shy", 60);
        shy.character = profile_with(|p| {
            p.big_matches = 1;
            p.pressure_handling = 1;
        });
        let neutral = player("n", 60);
        for p in [&big, &shy, &neutral] {
            assert_eq!(character_match_factor(p, false, false), 1.0);
            for (big_match, pressure) in [(true, false), (false, true), (true, true)] {
                let factor = character_match_factor(p, big_match, pressure);
                assert!((0.96..=1.04).contains(&factor), "{factor}");
            }
        }
        assert!(character_match_factor(&big, true, true) > 1.04 - 1e-9);
        assert!(character_match_factor(&shy, true, true) < 0.96 + 1e-9);
        assert!((character_match_factor(&neutral, true, true) - 1.0).abs() < 1e-12);
    }
}
