//! What a side brings to a match beyond its eleven and its tactics: its captain and vice-captain,
//! its set-piece takers and its manager's team talk. Both engines read them through here, so the
//! instant and the live match play them by one rule.
//!
//! Every effect is neutral for a side that has set nothing: no captain named, no taker named, a
//! `Neutral` talk. Such a side plays bit for bit as before, and draws nothing more from the
//! match's random stream.

use serde::{Deserialize, Serialize};

use crate::live_match::SetPieceTakers;
use crate::shared::PlayerSnap;
use crate::types::{PlayerData, Side, TeamTalkTone};
use crate::xg::ShotBodyPart;

/// A side's captaincy, set-piece takers and team talk, for `simulate_with_extras`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TeamExtras {
    #[serde(default)]
    pub set_pieces: SetPieceTakers,
    #[serde(default)]
    pub talk: TeamTalkTone,
}

/// The player who leads `players` on the pitch: the captain while he is on it, else the
/// vice-captain while he is, else nobody. A side whose named leaders have both gone (sent off,
/// substituted) has lost what they brought.
pub(crate) fn acting_captain<'a>(
    players: &'a [PlayerData],
    set_pieces: &SetPieceTakers,
    on_the_pitch: impl Fn(&str) -> bool,
) -> Option<&'a PlayerData> {
    [&set_pieces.captain, &set_pieces.vice_captain]
        .into_iter()
        .flatten()
        .filter(|id| on_the_pitch(id))
        .find_map(|id| players.iter().find(|player| player.id == *id))
}

/// The designated taker in `designated`, while he is on the pitch.
pub(crate) fn taker_on_the_pitch(
    players: &[PlayerData],
    designated: Option<&str>,
    on_the_pitch: impl Fn(&str) -> bool,
) -> Option<PlayerSnap> {
    let id = designated.filter(|id| on_the_pitch(id))?;
    players
        .iter()
        .find(|player| player.id == id)
        .map(PlayerSnap::from)
}

/// Leadership as a lean either side of the average player: -1 at 0, 0 at 50, +1 at 100.
fn leadership_lean(leadership: u8) -> f64 {
    ((f64::from(leadership) - 50.0) / 50.0).clamp(-1.0, 1.0)
}

/// How much steadier a side plays with its captain on the pitch: a few percent on its players'
/// ratings, more when it is under pressure (behind, or away from home), where a leader counts
/// most. A poor leader unsettles as a good one steadies. 1.0 with no captain on the pitch.
pub(crate) fn captain_composure(leadership: Option<u8>, under_pressure: bool) -> f64 {
    let Some(leadership) = leadership else {
        return 1.0;
    };
    let reach = if under_pressure { 0.04 } else { 0.015 };
    1.0 + leadership_lean(leadership) * reach
}

/// The factor on a side's fouls and cards for its captain: a strong leader keeps heads cool, a
/// weak one lets them go. 0.94 to 1.06; 1.0 with no captain on the pitch.
pub(crate) fn captain_discipline(leadership: Option<u8>) -> f64 {
    leadership.map_or(1.0, |leadership| 1.0 - leadership_lean(leadership) * 0.06)
}

/// What a team talk does to a side, as factors on its players' ratings (`morale`), on its fouls
/// (`aggression`) and on how fast it tires (`fatigue`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct TalkEffect {
    pub morale: f64,
    pub aggression: f64,
    pub fatigue: f64,
}

impl TalkEffect {
    const NEUTRAL: Self = Self {
        morale: 1.0,
        aggression: 1.0,
        fatigue: 1.0,
    };
}

/// How long a fired-up side's edge lasts, in minutes after the talk.
const FIRE_UP_MINUTES: f64 = 30.0;

/// What `tone` does to a side `goal_difference` up (negative: down), `minutes_since` the talk.
/// Each tone suits a situation and backfires a little outside it, so none is a free lift; every
/// factor stays within 0.95 and 1.05.
pub(crate) fn team_talk_effect(
    tone: TeamTalkTone,
    goal_difference: i16,
    minutes_since: u8,
) -> TalkEffect {
    let (ahead, level) = (goal_difference > 0, goal_difference == 0);
    match tone {
        TeamTalkTone::Neutral => TalkEffect::NEUTRAL,
        TeamTalkTone::Encourage => TalkEffect {
            morale: if ahead { 1.01 } else { 1.02 },
            ..TalkEffect::NEUTRAL
        },
        TeamTalkTone::DemandMore => TalkEffect {
            morale: if ahead {
                0.97
            } else if level {
                1.01
            } else {
                1.03
            },
            aggression: 1.04,
            fatigue: 1.02,
        },
        TeamTalkTone::Calm => TalkEffect {
            morale: if ahead { 1.02 } else { 0.99 },
            aggression: 0.95,
            fatigue: 0.98,
        },
        TeamTalkTone::FireUp => {
            let edge = (1.0 - f64::from(minutes_since) / FIRE_UP_MINUTES).max(0.0);
            TalkEffect {
                // Sharp straight after the talk; a little flat once the edge has worn off.
                morale: 0.99 + 0.06 * edge,
                aggression: 1.05,
                fatigue: 1.04,
            }
        }
        TeamTalkTone::Praise => TalkEffect {
            morale: if ahead {
                1.03
            } else if level {
                1.01
            } else {
                0.96
            },
            aggression: 0.98,
            ..TalkEffect::NEUTRAL
        },
    }
}

/// Whether `side` is playing under pressure at `goal_difference`: behind, or away from home.
pub(crate) fn under_pressure(side: Side, goal_difference: i16) -> bool {
    goal_difference < 0 || side == Side::Away
}

/// The trait that makes a player a set-piece specialist.
const SET_PIECE_SPECIALIST: &str = "SetPieceSpecialist";

/// How good a free kick or corner a taker puts into the box, as a factor on the chance it makes:
/// 1.0 for a taker of average delivery (65), up to a quarter better for a specialist with the best
/// passing and vision, a sixth worse for a poor one.
pub(crate) fn delivery_quality(taker: &PlayerSnap) -> f64 {
    let skill = f64::from(taker.passing) * 0.6 + f64::from(taker.vision) * 0.4;
    let specialist = if taker.has_trait(SET_PIECE_SPECIALIST) {
        1.06
    } else {
        1.0
    };
    ((1.0 + (skill - 65.0) / 150.0).clamp(0.85, 1.2) * specialist).min(1.25)
}

/// A taker's skill at a direct free kick, on the attribute scale.
pub(crate) fn free_kick_skill(taker: &PlayerSnap) -> f64 {
    let skill = f64::from(taker.shooting) * 0.5
        + f64::from(taker.passing) * 0.25
        + f64::from(taker.vision) * 0.25;
    if taker.has_trait(SET_PIECE_SPECIALIST) {
        skill * 1.08
    } else {
        skill
    }
}

/// The share of free kicks won in the attacking third that are close enough to shoot from:
/// most are wide or deep, and a striker of the ball puts those into the box like anyone else.
pub(crate) const FREE_KICK_IN_SHOOTING_RANGE: f64 = 0.25;

/// Whether a taker goes for goal from a free kick rather than putting it into the box: a striker
/// of the ball goes for goal, a passer crosses it.
pub(crate) fn shoots_free_kicks(taker: &PlayerSnap) -> bool {
    taker.shooting >= taker.passing
}

/// The chance a direct free kick is on target, from the taker's skill: about a quarter for a
/// good taker, as in the top leagues.
pub(crate) fn free_kick_accuracy(skill: f64) -> f64 {
    (0.15 + (skill - 50.0) / 200.0).clamp(0.10, 0.45)
}

/// The chance an on-target direct free kick beats the keeper. With the accuracy, a good taker
/// scores about one in twenty-five, a specialist one in twelve: direct free kicks return about 5%.
pub(crate) fn free_kick_conversion(skill: f64, keeper_rating: f64) -> f64 {
    (0.12 + (skill - keeper_rating) / 200.0).clamp(0.03, 0.30)
}

/// The chance a free kick crossed into the box finds a team-mate there.
pub(crate) fn free_kick_delivery_chance(quality: f64) -> f64 {
    (0.45 * quality).clamp(0.3, 0.6)
}

/// The base chance a corner won in the attacking third is played into the box.
pub(crate) const CORNER_INTO_THE_BOX: f64 = 0.30;

/// A set-piece header is a contested chance, made by a ball into a crowded box: it plays at this
/// share of a shot a midfielder worked out of open play. Without it a taker's delivery (up to
/// 1.25) stood in for the creating midfielder's factor in full, and a side with designated takers
/// scored about 0.35 goals a game more than one without (`tactical_adaptation_probe` read 2.12
/// against 1.76): a set piece made more than open play did. Tuned with [`DELIVERY_SHARE`] so a
/// side with takers scores about what one without does (8 seeds: 1.75 against 1.77 goals a game),
/// the taker deciding who takes the kick and how good it is rather than how many goals there are.
pub(crate) const SET_PIECE_HEADER: f64 = 0.80;

/// The share of a taker's delivery above an ordinary ball that reaches the header: half, so the
/// best taker (quality 1.25) lifts a header by an eighth, not a quarter.
pub(crate) const DELIVERY_SHARE: f64 = 0.5;

/// A set piece delivered into the box, waiting for the shot it makes.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PendingSetPiece {
    pub side: Side,
    /// Who delivered it, if the side had a taker on the pitch.
    pub taker_id: Option<String>,
    /// The delivery's quality (`delivery_quality`); 1.0 with no designated taker.
    pub quality: f64,
}

/// How the chance a shot comes from was made.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ShotOrigin {
    /// Worked into the box on the ground.
    OpenPlay,
    /// Headed from a cross.
    Cross,
    /// Headed from a corner or free kick put into the box.
    SetPiece(PendingSetPiece),
}

impl ShotOrigin {
    /// The shot a set piece makes for `side`, if `pending` is one of its own; open play otherwise.
    pub(crate) fn in_the_box(pending: Option<PendingSetPiece>, side: Side) -> Self {
        match pending {
            Some(set_piece) if set_piece.side == side => Self::SetPiece(set_piece),
            _ => Self::OpenPlay,
        }
    }

    pub(crate) fn body_part(&self) -> ShotBodyPart {
        match self {
            Self::OpenPlay => ShotBodyPart::Foot,
            Self::Cross | Self::SetPiece(_) => ShotBodyPart::Header,
        }
    }

    /// The factor on the shot for the delivery: 1.0 but for a designated taker's set piece, where
    /// it stands in for the creating midfielder (`resolve_shot`): a contested header
    /// ([`SET_PIECE_HEADER`]) lifted by [`DELIVERY_SHARE`] of the taker's delivery above 1.0.
    pub(crate) fn delivery(&self) -> f64 {
        match self {
            Self::SetPiece(set_piece) => {
                SET_PIECE_HEADER * (1.0 + (set_piece.quality - 1.0) * DELIVERY_SHARE)
            }
            Self::OpenPlay | Self::Cross => 1.0,
        }
    }

    /// The designated taker who made the chance, if one did.
    pub(crate) fn taker(&self) -> Option<&str> {
        match self {
            Self::SetPiece(set_piece) => set_piece.taker_id.as_deref(),
            Self::OpenPlay | Self::Cross => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::player;
    use crate::types::Position;

    fn snap(change: impl FnOnce(&mut PlayerData)) -> PlayerSnap {
        let mut taker = player("t", Position::Midfielder);
        change(&mut taker);
        PlayerSnap::from(&taker)
    }

    fn takers(captain: Option<&str>, vice: Option<&str>) -> SetPieceTakers {
        SetPieceTakers {
            captain: captain.map(str::to_string),
            vice_captain: vice.map(str::to_string),
            ..SetPieceTakers::default()
        }
    }

    fn set_piece(quality: f64) -> ShotOrigin {
        ShotOrigin::SetPiece(PendingSetPiece {
            side: Side::Home,
            taker_id: Some("t".to_string()),
            quality,
        })
    }

    /// Given a set piece put in by an ordinary taker and one by the best,
    /// When the shot it makes reads its delivery,
    /// Then it is a contested header (0.80) lifted by half the delivery above 1.0 (0.90 at
    ///      1.25), below the open-play midfielder's 1.0: a set piece makes less than open play.
    #[test]
    fn a_set_piece_header_is_contested_and_lifted_by_half_the_delivery() {
        assert!((set_piece(1.0).delivery() - 0.80).abs() < 1e-12);
        assert!((set_piece(1.25).delivery() - 0.90).abs() < 1e-12);
        assert_eq!(ShotOrigin::OpenPlay.delivery(), 1.0);
        assert_eq!(ShotOrigin::Cross.delivery(), 1.0);
    }

    /// Given a captain and a vice-captain both on the pitch,
    /// When the acting captain is read, Then it is the captain.
    #[test]
    fn the_captain_leads_while_he_is_on_the_pitch() {
        let players = vec![
            player("c", Position::Defender),
            player("v", Position::Forward),
        ];
        let leader = acting_captain(&players, &takers(Some("c"), Some("v")), |_| true);
        assert_eq!(leader.map(|p| p.id.as_str()), Some("c"));
    }

    /// Given the captain gone from the pitch,
    /// When the acting captain is read, Then the vice-captain leads.
    #[test]
    fn the_vice_captain_leads_once_the_captain_has_gone() {
        let players = vec![
            player("c", Position::Defender),
            player("v", Position::Forward),
        ];
        let leader = acting_captain(&players, &takers(Some("c"), Some("v")), |id| id != "c");
        assert_eq!(leader.map(|p| p.id.as_str()), Some("v"));
    }

    /// Given both named leaders gone, or none named,
    /// When the acting captain is read, Then nobody leads.
    #[test]
    fn nobody_leads_once_both_have_gone_or_none_was_named() {
        let players = vec![
            player("c", Position::Defender),
            player("v", Position::Forward),
        ];
        assert!(acting_captain(&players, &takers(Some("c"), Some("v")), |_| false).is_none());
        assert!(acting_captain(&players, &takers(None, None), |_| true).is_none());
    }

    /// Given no captain on the pitch,
    /// When his effects are read, Then they are exactly neutral.
    #[test]
    fn no_captain_is_neutral() {
        assert_eq!(captain_composure(None, true), 1.0);
        assert_eq!(captain_discipline(None), 1.0);
    }

    /// Given a strong leader and a weak one,
    /// When their effects are read, Then the strong one steadies the side more under pressure
    /// than not, and cuts its fouls; the weak one does the opposite.
    #[test]
    fn a_strong_leader_steadies_and_a_weak_one_unsettles() {
        assert!(captain_composure(Some(95), true) > captain_composure(Some(95), false));
        assert!(captain_composure(Some(95), false) > 1.0);
        assert!(captain_composure(Some(10), true) < 1.0);
        assert!(captain_discipline(Some(95)) < 1.0);
        assert!(captain_discipline(Some(10)) > 1.0);
    }

    /// Given every tone, score and minute,
    /// When the talk's effect is read, Then every factor is within 0.95 and 1.05, and Neutral is
    /// exactly neutral.
    #[test]
    fn every_talk_is_bounded_and_neutral_is_neutral() {
        assert_eq!(
            team_talk_effect(TeamTalkTone::Neutral, -2, 0),
            TalkEffect::NEUTRAL
        );
        for tone in [
            TeamTalkTone::Encourage,
            TeamTalkTone::DemandMore,
            TeamTalkTone::Calm,
            TeamTalkTone::FireUp,
            TeamTalkTone::Praise,
        ] {
            for goal_difference in -3..=3 {
                for minutes_since in [0, 10, 30, 90] {
                    let effect = team_talk_effect(tone, goal_difference, minutes_since);
                    for factor in [effect.morale, effect.aggression, effect.fatigue] {
                        assert!((0.95..=1.05).contains(&factor), "{tone:?}: {effect:?}");
                    }
                }
            }
        }
    }

    /// Given a fired-up side,
    /// When its edge is read straight after the talk and an hour later,
    /// Then it is sharp at first and has worn off later, and it fouls and tires more throughout.
    #[test]
    fn firing_up_a_side_wears_off_and_costs_fouls_and_legs() {
        let early = team_talk_effect(TeamTalkTone::FireUp, 0, 0);
        let late = team_talk_effect(TeamTalkTone::FireUp, 0, 60);
        assert!(early.morale > late.morale);
        assert!(early.aggression > 1.0 && early.fatigue > 1.0);
    }

    /// Given a calm talk, When read, Then the side fouls less.
    #[test]
    fn a_calm_talk_cools_the_tackling() {
        assert!(team_talk_effect(TeamTalkTone::Calm, 0, 0).aggression < 1.0);
    }

    /// Given a poor deliverer, an average one and a specialist with fine passing,
    /// When their delivery is rated, Then each is better than the last.
    #[test]
    fn delivery_improves_with_passing_vision_and_the_specialist_trait() {
        let poor = delivery_quality(&snap(|p| (p.passing, p.vision) = (35, 35)));
        let average = delivery_quality(&snap(|p| (p.passing, p.vision) = (65, 65)));
        let specialist = delivery_quality(&snap(|p| {
            (p.passing, p.vision) = (90, 85);
            p.traits = vec![SET_PIECE_SPECIALIST.to_string()];
        }));
        assert!(
            poor < average && average < specialist,
            "{poor} {average} {specialist}"
        );
        assert!((average - 1.0).abs() < 1e-9);
    }

    /// Given a designated taker who is on the pitch, and one who is not,
    /// When the taker is resolved, Then only the one on the pitch takes it.
    #[test]
    fn a_taker_takes_set_pieces_only_from_the_pitch() {
        let players = vec![player("t", Position::Midfielder)];
        assert!(taker_on_the_pitch(&players, Some("t"), |_| true).is_some());
        assert!(taker_on_the_pitch(&players, Some("t"), |_| false).is_none());
        assert!(taker_on_the_pitch(&players, None, |_| true).is_none());
    }
}
