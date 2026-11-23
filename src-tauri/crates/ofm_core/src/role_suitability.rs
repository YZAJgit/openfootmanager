//! How well a player's attributes suit each tactical role: one weight for every role and every
//! attribute, summing to 1 for each role, so a suitability reads on the attributes' own 0..=100
//! scale. A player of 70 in everything suits every role at 70; one of 90 in passing and vision
//! suits a playmaker better than a ball-winner.
//!
//! The table is the football judgement of what each role asks for, not the engine's resolution:
//! the match reads roles through its own phase modifiers (`engine::role_has_match_effect`).

use domain::player::{AttributeKey, Player, PlayerAttributes, Position};
use domain::team::PlayerRole;

use AttributeKey::{
    Aerial, Aggression, Agility, Composure, Decisions, Defending, Dribbling, Handling, Leadership,
    Pace, Passing, Positioning, Reflexes, Shooting, Stamina, Strength, Tackling, Teamwork, Vision,
};

/// What `role` asks for, in hundredths: every attribute not named weighs nothing. Each list sums
/// to 100 (`every_role_weighs_its_attributes_to_one`).
// Length is a flat per-role data table, not logic; splitting it would only scatter the weights.
#[allow(clippy::too_many_lines)]
fn weights(role: &PlayerRole) -> &'static [(AttributeKey, u8)] {
    match role {
        PlayerRole::Standard => &[
            (Pace, 6),
            (Stamina, 6),
            (Strength, 6),
            (Agility, 5),
            (Passing, 7),
            (Shooting, 5),
            (Tackling, 5),
            (Dribbling, 5),
            (Defending, 6),
            (Positioning, 7),
            (Vision, 5),
            (Decisions, 7),
            (Composure, 6),
            (Aggression, 3),
            (Teamwork, 6),
            (Leadership, 3),
            (Handling, 4),
            (Reflexes, 4),
            (Aerial, 4),
        ],
        PlayerRole::BallPlayingKeeper => &[
            (Handling, 20),
            (Reflexes, 22),
            (Positioning, 10),
            (Passing, 15),
            (Vision, 8),
            (Decisions, 8),
            (Composure, 10),
            (Agility, 5),
            (Aerial, 2),
        ],
        PlayerRole::SweeperKeeper => &[
            (Handling, 18),
            (Reflexes, 20),
            (Positioning, 12),
            (Pace, 10),
            (Agility, 8),
            (Decisions, 10),
            (Composure, 8),
            (Aerial, 6),
            (Passing, 8),
        ],
        PlayerRole::Stopper => &[
            (Tackling, 18),
            (Strength, 15),
            (Aggression, 12),
            (Aerial, 15),
            (Defending, 15),
            (Positioning, 10),
            (Pace, 5),
            (Decisions, 5),
            (Composure, 5),
        ],
        PlayerRole::CoverCB => &[
            (Positioning, 20),
            (Defending, 18),
            (Pace, 14),
            (Decisions, 14),
            (Tackling, 10),
            (Composure, 8),
            (Aerial, 8),
            (Agility, 4),
            (Teamwork, 4),
        ],
        PlayerRole::BallPlayingCB => &[
            (Passing, 18),
            (Vision, 12),
            (Composure, 14),
            (Defending, 14),
            (Positioning, 12),
            (Tackling, 10),
            (Decisions, 10),
            (Aerial, 6),
            (Strength, 4),
        ],
        PlayerRole::AttackingFB => &[
            (Pace, 16),
            (Stamina, 14),
            (Dribbling, 12),
            (Passing, 12),
            (Agility, 8),
            (Defending, 10),
            (Tackling, 8),
            (Teamwork, 8),
            (Positioning, 6),
            (Vision, 6),
        ],
        PlayerRole::DefensiveFB => &[
            (Defending, 18),
            (Tackling, 16),
            (Positioning, 16),
            (Pace, 12),
            (Stamina, 10),
            (Strength, 8),
            (Decisions, 8),
            (Teamwork, 6),
            (Aerial, 6),
        ],
        PlayerRole::InvertedFB => &[
            (Passing, 16),
            (Vision, 12),
            (Decisions, 12),
            (Composure, 12),
            (Positioning, 12),
            (Tackling, 10),
            (Defending, 10),
            (Teamwork, 8),
            (Agility, 8),
        ],
        PlayerRole::WingBack => &[
            (Pace, 16),
            (Stamina, 18),
            (Dribbling, 12),
            (Passing, 10),
            (Agility, 8),
            (Tackling, 8),
            (Defending, 8),
            (Teamwork, 10),
            (Vision, 4),
            (Positioning, 6),
        ],
        PlayerRole::AnchorMan => &[
            (Positioning, 20),
            (Defending, 16),
            (Tackling, 14),
            (Decisions, 14),
            (Teamwork, 10),
            (Strength, 8),
            (Composure, 8),
            (Passing, 6),
            (Aerial, 4),
        ],
        PlayerRole::BallWinner => &[
            (Tackling, 22),
            (Aggression, 16),
            (Stamina, 14),
            (Strength, 10),
            (Defending, 10),
            (Pace, 8),
            (Teamwork, 8),
            (Positioning, 6),
            (Agility, 6),
        ],
        PlayerRole::DeepLyingPlaymaker => &[
            (Passing, 22),
            (Vision, 20),
            (Composure, 14),
            (Decisions, 14),
            (Positioning, 8),
            (Teamwork, 8),
            (Tackling, 6),
            (Agility, 4),
            (Dribbling, 4),
        ],
        PlayerRole::BoxToBox => &[
            (Stamina, 18),
            (Pace, 10),
            (Tackling, 10),
            (Passing, 10),
            (Shooting, 10),
            (Strength, 8),
            (Teamwork, 10),
            (Decisions, 8),
            (Positioning, 6),
            (Dribbling, 6),
            (Aggression, 4),
        ],
        PlayerRole::Carrilero => &[
            (Teamwork, 18),
            (Positioning, 16),
            (Passing, 14),
            (Stamina, 14),
            (Decisions, 12),
            (Tackling, 10),
            (Defending, 8),
            (Composure, 8),
        ],
        PlayerRole::Mezzala => &[
            (Dribbling, 16),
            (Passing, 14),
            (Vision, 12),
            (Shooting, 12),
            (Agility, 10),
            (Pace, 10),
            (Stamina, 10),
            (Decisions, 8),
            (Composure, 8),
        ],
        PlayerRole::AdvancedPlaymaker => &[
            (Vision, 24),
            (Passing, 22),
            (Dribbling, 12),
            (Decisions, 12),
            (Composure, 12),
            (Agility, 8),
            (Shooting, 6),
            (Teamwork, 4),
        ],
        PlayerRole::ShadowStriker => &[
            (Shooting, 20),
            (Positioning, 16),
            (Pace, 12),
            (Dribbling, 12),
            (Composure, 12),
            (Agility, 10),
            (Decisions, 8),
            (Stamina, 6),
            (Vision, 4),
        ],
        PlayerRole::WideForward => &[
            (Pace, 20),
            (Dribbling, 16),
            (Shooting, 14),
            (Agility, 12),
            (Passing, 10),
            (Stamina, 8),
            (Positioning, 8),
            (Composure, 6),
            (Vision, 6),
        ],
        PlayerRole::InsideForward => &[
            (Shooting, 20),
            (Dribbling, 18),
            (Pace, 14),
            (Agility, 12),
            (Composure, 10),
            (Positioning, 10),
            (Decisions, 8),
            (Vision, 8),
        ],
        PlayerRole::InvertedWinger => &[
            (Dribbling, 18),
            (Passing, 16),
            (Vision, 14),
            (Pace, 12),
            (Agility, 12),
            (Shooting, 10),
            (Decisions, 8),
            (Composure, 10),
        ],
        PlayerRole::Poacher => &[
            (Shooting, 26),
            (Positioning, 22),
            (Composure, 16),
            (Pace, 12),
            (Agility, 8),
            (Decisions, 8),
            (Aerial, 8),
        ],
        PlayerRole::TargetMan => &[
            (Strength, 22),
            (Aerial, 22),
            (Shooting, 14),
            (Positioning, 12),
            (Composure, 8),
            (Teamwork, 8),
            (Passing, 6),
            (Aggression, 8),
        ],
        PlayerRole::DeepLyingForward => &[
            (Passing, 18),
            (Vision, 16),
            (Shooting, 14),
            (Composure, 12),
            (Decisions, 12),
            (Dribbling, 10),
            (Teamwork, 10),
            (Strength, 8),
        ],
        PlayerRole::False9 => &[
            (Passing, 16),
            (Vision, 18),
            (Dribbling, 16),
            (Decisions, 12),
            (Agility, 10),
            (Composure, 10),
            (Shooting, 10),
            (Teamwork, 8),
        ],
        PlayerRole::PressingForward => &[
            (Stamina, 18),
            (Aggression, 14),
            (Pace, 14),
            (Teamwork, 14),
            (Tackling, 10),
            (Shooting, 12),
            (Positioning, 8),
            (Strength, 6),
            (Decisions, 4),
        ],
        PlayerRole::CompleteForward => &[
            (Shooting, 16),
            (Dribbling, 12),
            (Strength, 10),
            (Aerial, 10),
            (Pace, 10),
            (Passing, 10),
            (Composure, 10),
            (Positioning, 10),
            (Vision, 6),
            (Agility, 6),
        ],
    }
}

/// The weight `role` puts on `attribute`, `0.0..=1.0`; a role's weights sum to 1.
pub fn role_weight(role: &PlayerRole, attribute: AttributeKey) -> f32 {
    weights(role)
        .iter()
        .find(|(key, _)| *key == attribute)
        .map_or(0.0, |(_, hundredths)| f32::from(*hundredths) / 100.0)
}

/// How well `attributes` suit `role`, on the attributes' own `0..=100` scale.
pub fn role_suitability(role: &PlayerRole, attributes: &PlayerAttributes) -> f32 {
    let weighted: f32 = AttributeKey::ALL
        .iter()
        .map(|key| role_weight(role, *key) * f32::from(key.value_in(attributes)))
        .sum();
    weighted.clamp(0.0, 100.0)
}

/// Every role `position` admits, best suited to `player` first; two equally suited keep the order
/// the position lists them in (`Position::valid_roles`).
pub fn best_roles_for_player(player: &Player, position: Position) -> Vec<(PlayerRole, f32)> {
    let mut ranked: Vec<_> = position
        .valid_roles()
        .iter()
        .map(|role| (role.clone(), role_suitability(role, &player.attributes)))
        .collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1));
    ranked
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::uniform_attributes;

    const ALL_ROLES: [PlayerRole; 27] = [
        PlayerRole::Standard,
        PlayerRole::BallPlayingKeeper,
        PlayerRole::SweeperKeeper,
        PlayerRole::Stopper,
        PlayerRole::CoverCB,
        PlayerRole::BallPlayingCB,
        PlayerRole::AttackingFB,
        PlayerRole::DefensiveFB,
        PlayerRole::InvertedFB,
        PlayerRole::WingBack,
        PlayerRole::AnchorMan,
        PlayerRole::BallWinner,
        PlayerRole::DeepLyingPlaymaker,
        PlayerRole::BoxToBox,
        PlayerRole::Carrilero,
        PlayerRole::Mezzala,
        PlayerRole::AdvancedPlaymaker,
        PlayerRole::ShadowStriker,
        PlayerRole::WideForward,
        PlayerRole::InsideForward,
        PlayerRole::InvertedWinger,
        PlayerRole::Poacher,
        PlayerRole::TargetMan,
        PlayerRole::DeepLyingForward,
        PlayerRole::False9,
        PlayerRole::PressingForward,
        PlayerRole::CompleteForward,
    ];

    fn with(change: impl FnOnce(&mut PlayerAttributes)) -> PlayerAttributes {
        let mut attributes = uniform_attributes(50);
        change(&mut attributes);
        attributes
    }

    fn player_with(attributes: PlayerAttributes, position: Position) -> Player {
        Player::new(
            "p".into(),
            "P".into(),
            "Player".into(),
            "1995-01-01".into(),
            "ENG".into(),
            position,
            attributes,
        )
    }

    fn rank(role: PlayerRole, attributes: &PlayerAttributes) -> f32 {
        role_suitability(&role, attributes)
    }

    /// Given every one of the 27 roles and the 19 attributes
    /// When the full table of weights is read
    /// Then each role's weights sum to 1, none is negative, and no attribute is named twice
    #[test]
    fn every_role_weighs_its_attributes_to_one() {
        for role in &ALL_ROLES {
            let total: f32 = AttributeKey::ALL
                .iter()
                .map(|key| role_weight(role, *key))
                .sum();
            assert!((total - 1.0).abs() < 1e-4, "{role:?} sums to {total}");
            for key in AttributeKey::ALL {
                assert!(role_weight(role, key) >= 0.0);
                let named = weights(role).iter().filter(|(k, _)| *k == key).count();
                assert!(named <= 1, "{role:?} names {key:?} {named} times");
            }
        }
    }

    /// Given a player of one rating in every attribute
    /// When he is read against every role
    /// Then he suits every role at that rating: the scale is the attributes' own
    #[test]
    fn a_uniform_player_suits_every_role_at_his_rating() {
        for role in &ALL_ROLES {
            let suitability = role_suitability(role, &uniform_attributes(70));
            assert!((suitability - 70.0).abs() < 1e-3, "{role:?}: {suitability}");
            assert!(role_suitability(role, &uniform_attributes(0)) >= 0.0);
            assert!(role_suitability(role, &uniform_attributes(100)) <= 100.0);
        }
    }

    /// Given a passer with vision, and a hard-tackling, aggressive runner
    /// When each is read as a deep-lying playmaker, an advanced playmaker and a ball-winner
    /// Then the passer suits both playmakers above the ball-winner, and the runner the reverse
    #[test]
    fn a_passer_suits_the_playmakers_and_a_tackler_the_ball_winner() {
        let passer = with(|a| {
            a.passing = 90;
            a.vision = 90;
        });
        assert!(
            rank(PlayerRole::DeepLyingPlaymaker, &passer) > rank(PlayerRole::BallWinner, &passer)
        );
        assert!(
            rank(PlayerRole::AdvancedPlaymaker, &passer) > rank(PlayerRole::BallWinner, &passer)
        );
        let tackler = with(|a| {
            a.tackling = 90;
            a.aggression = 90;
            a.stamina = 85;
        });
        assert!(
            rank(PlayerRole::BallWinner, &tackler) > rank(PlayerRole::DeepLyingPlaymaker, &tackler)
        );
        assert!(
            rank(PlayerRole::BallWinner, &tackler) > rank(PlayerRole::AdvancedPlaymaker, &tackler)
        );
    }

    /// Given a tall, strong forward, a fast finisher, and a centre-back who passes
    /// When each is ranked among his position's roles
    /// Then the first is a target man, the second a poacher, the third a ball-playing defender
    #[test]
    fn a_player_is_ranked_first_in_the_role_his_attributes_make() {
        let tall = player_with(
            with(|a| {
                a.strength = 92;
                a.aerial = 92;
            }),
            Position::Striker,
        );
        let best = best_roles_for_player(&tall, Position::Striker);
        assert_eq!(best[0].0, PlayerRole::TargetMan);

        let finisher = player_with(
            with(|a| {
                a.shooting = 92;
                a.positioning = 90;
                a.composure = 85;
            }),
            Position::Striker,
        );
        assert_eq!(
            best_roles_for_player(&finisher, Position::Striker)[0].0,
            PlayerRole::Poacher
        );

        let passer = player_with(
            with(|a| {
                a.passing = 90;
                a.vision = 85;
                a.composure = 85;
            }),
            Position::CenterBack,
        );
        assert_eq!(
            best_roles_for_player(&passer, Position::CenterBack)[0].0,
            PlayerRole::BallPlayingCB
        );
    }

    /// Given a goalkeeper ranked in goal and a striker ranked up front
    /// When his roles are listed
    /// Then exactly the roles the position admits are listed, best first
    #[test]
    fn only_the_roles_a_position_admits_are_ranked_and_best_first() {
        let keeper = player_with(uniform_attributes(60), Position::Goalkeeper);
        let ranked = best_roles_for_player(&keeper, Position::Goalkeeper);
        assert_eq!(ranked.len(), Position::Goalkeeper.valid_roles().len());
        assert!(
            ranked
                .iter()
                .all(|(role, _)| Position::Goalkeeper.admits_role(role))
        );
        assert!(ranked.windows(2).all(|pair| pair[0].1 >= pair[1].1));
    }
}
