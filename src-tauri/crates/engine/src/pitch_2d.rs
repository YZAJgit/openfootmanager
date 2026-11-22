//! A two-dimensional picture of the match, minute by minute, for a pitch view.
//!
//! The engine plays the match in five zones, not in metres; this places each player where his
//! side's shape puts him given where the ball is: the line he plays in, how deep the side defends,
//! how wide it plays, how compact its block is, whether it has the ball, and who acted last. It is
//! a pure function of the match state, so it never changes a result and draws nothing from the
//! match's random stream.
//!
//! Coordinates: `x` 0 to 100 from the home goal to the away goal, `y` 0 to 100 across. The home
//! side defends `x = 0` and attacks `x = 100`; the away side the reverse.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::event::{EventType, MatchEvent};
use crate::types::{
    DefensiveLine, DefensiveShape, PlayerData, Position, Side, TacticsPitchWidth, TeamData, Zone,
};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, ts_rs::TS)]
pub struct PitchCoord {
    pub x: f32,
    pub y: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, ts_rs::TS)]
pub struct PlayerDot2D {
    pub player_id: String,
    pub side: Side,
    #[ts(type = "string")]
    pub position: Position,
    pub x: f32,
    pub y: f32,
    pub has_ball: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, ts_rs::TS)]
pub struct BallState2D {
    pub x: f32,
    pub y: f32,
    pub owner_id: Option<String>,
    pub carrier_side: Side,
    pub in_flight: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, ts_rs::TS)]
pub struct PitchFrame2D {
    pub minute: u8,
    pub phase: String,
    pub ball: BallState2D,
    pub players: Vec<PlayerDot2D>,
    pub last_event_label: Option<String>,
}

/// The match as a frame is drawn from it.
pub(crate) struct FrameInput<'a> {
    pub minute: u8,
    pub phase: String,
    pub home: &'a TeamData,
    pub away: &'a TeamData,
    /// The players who have left the pitch (sent off). A substituted-off player is no longer in
    /// his side's players.
    pub off_the_pitch: &'a HashSet<String>,
    pub ball_zone: Zone,
    pub possession: Side,
    pub last_event: Option<&'a MatchEvent>,
}

/// The closest two dots may stand before one is moved aside.
const MIN_SEPARATION: f32 = 1.0;
const EDGE: f32 = 1.0;

/// The frame for the match as `input` has it.
pub(crate) fn frame(input: &FrameInput<'_>) -> PitchFrame2D {
    let mut players = Vec::with_capacity(22);
    for side in [Side::Home, Side::Away] {
        let team = match side {
            Side::Home => input.home,
            Side::Away => input.away,
        };
        players.extend(side_dots(team, side, input));
    }

    let owner_index = input
        .last_event
        .filter(|event| event.side == input.possession)
        .and_then(|event| event.player_id.as_deref())
        .and_then(|id| players.iter().position(|dot| dot.player_id == id));
    let zone_x = zone_centre_x(input.ball_zone);
    let ball = match owner_index {
        Some(index) => {
            let owner = &mut players[index];
            // The man on the ball is where the ball is, not where his shape would put him.
            owner.x = clamp_on_pitch((owner.x + zone_x) / 2.0);
            owner.has_ball = true;
            let ahead = match owner.side {
                Side::Home => 1.0,
                Side::Away => -1.0,
            };
            BallState2D {
                x: clamp_on_pitch(owner.x + ahead),
                y: owner.y,
                owner_id: Some(owner.player_id.clone()),
                carrier_side: input.possession,
                in_flight: false,
            }
        }
        None => BallState2D {
            x: zone_x,
            y: 50.0,
            owner_id: None,
            carrier_side: input.possession,
            in_flight: input
                .last_event
                .is_some_and(|event| ball_in_flight(&event.event_type)),
        },
    };
    separate(&mut players);

    PitchFrame2D {
        minute: input.minute,
        phase: input.phase.clone(),
        ball,
        players,
        last_event_label: input
            .last_event
            .map(|event| format!("{:?}", event.event_type)),
    }
}

/// A frame for every minute of a finished match, rebuilt from its event log and the two sides
/// that played it. For a report, which keeps the events but not the state minute by minute.
pub fn frames_from_events(
    home: &TeamData,
    away: &TeamData,
    events: &[MatchEvent],
) -> Vec<PitchFrame2D> {
    let mut frames = Vec::new();
    let mut sent_off = HashSet::new();
    let mut index = 0;
    let mut last: Option<&MatchEvent> = None;
    let Some(final_minute) = events.iter().map(|event| event.minute).max() else {
        return frames;
    };
    'minutes: for minute in 0..=final_minute {
        while let Some(event) = events.get(index).filter(|event| event.minute <= minute) {
            if matches!(
                event.event_type,
                EventType::RedCard | EventType::SecondYellow
            ) && let Some(id) = &event.player_id
            {
                sent_off.insert(id.clone());
            }
            if is_shootout(&event.event_type) {
                break 'minutes;
            }
            last = Some(event);
            index += 1;
        }
        let Some(event) = last else { continue };
        frames.push(frame(&FrameInput {
            minute,
            phase: half_of(minute).to_string(),
            home,
            away,
            off_the_pitch: &sent_off,
            ball_zone: event.zone,
            possession: event.side,
            last_event: Some(event),
        }));
    }
    frames
}

fn is_shootout(event_type: &EventType) -> bool {
    matches!(
        event_type,
        EventType::ShootoutGoal | EventType::ShootoutMiss
    )
}

/// A frame rebuilt from events knows the minute, not the phase: the half the minute falls in.
fn half_of(minute: u8) -> &'static str {
    match minute {
        0..=45 => "FirstHalf",
        46..=90 => "SecondHalf",
        91..=105 => "ExtraTimeFirstHalf",
        _ => "ExtraTimeSecondHalf",
    }
}

fn ball_in_flight(event_type: &EventType) -> bool {
    matches!(
        event_type,
        EventType::Cross
            | EventType::Corner
            | EventType::FreeKick
            | EventType::GoalKick
            | EventType::ShotOnTarget
            | EventType::ShotOffTarget
            | EventType::ShotSaved
            | EventType::Clearance
    )
}

/// The middle of a zone along the pitch.
fn zone_centre_x(zone: Zone) -> f32 {
    match zone {
        Zone::HomeBox => 8.0,
        Zone::HomeDefense => 25.0,
        Zone::Midfield => 50.0,
        Zone::AwayDefense => 75.0,
        Zone::AwayBox => 92.0,
    }
}

/// How far up the pitch the ball is from `side`'s point of view: -2 in its own box, +2 in the
/// opponent's.
fn ball_progress(zone: Zone, side: Side) -> f32 {
    let from_home = match zone {
        Zone::HomeBox => -2.0,
        Zone::HomeDefense => -1.0,
        Zone::Midfield => 0.0,
        Zone::AwayDefense => 1.0,
        Zone::AwayBox => 2.0,
    };
    match side {
        Side::Home => from_home,
        Side::Away => -from_home,
    }
}

/// Where a side's back line holds, from its own goal, with the ball at halfway.
fn back_line_depth(line: DefensiveLine) -> f32 {
    match line {
        DefensiveLine::VeryLow => 18.0,
        DefensiveLine::Low => 23.0,
        DefensiveLine::Medium => 28.0,
        DefensiveLine::High => 34.0,
    }
}

/// How spread the lines are front to back.
fn block_length(shape: DefensiveShape) -> f32 {
    match shape {
        DefensiveShape::Compact => 0.85,
        DefensiveShape::Normal => 1.0,
        DefensiveShape::Stretched => 1.15,
    }
}

/// How much of the pitch's width a line spreads across.
fn line_span(width: TacticsPitchWidth) -> f32 {
    match width {
        TacticsPitchWidth::Narrow => 50.0,
        TacticsPitchWidth::Normal => 64.0,
        TacticsPitchWidth::Wide => 76.0,
    }
}

/// The dots of one side's players still on the pitch.
fn side_dots(team: &TeamData, side: Side, input: &FrameInput<'_>) -> Vec<PlayerDot2D> {
    let on_the_pitch: Vec<&PlayerData> = team
        .players
        .iter()
        .filter(|player| !input.off_the_pitch.contains(&player.id))
        .collect();
    let progress = ball_progress(input.ball_zone, side);
    let in_possession = input.possession == side;
    let back = back_line_depth(team.tactics.defensive_line);
    let length = block_length(team.tactics.defensive_shape);
    let span = line_span(team.tactics.width);
    let shift = progress * 7.0 + if in_possession { 4.0 } else { -2.0 };

    let mut dots = Vec::with_capacity(on_the_pitch.len());
    for position in [
        Position::Goalkeeper,
        Position::Defender,
        Position::Midfielder,
        Position::Forward,
    ] {
        let line: Vec<&&PlayerData> = on_the_pitch
            .iter()
            .filter(|player| player.position == position)
            .collect();
        let depth = match position {
            Position::Goalkeeper => 5.0 + progress.max(0.0) * 3.0,
            Position::Defender => back + shift,
            Position::Midfielder => back + 20.0 * length + shift,
            Position::Forward => (back + 38.0 * length + shift).min(88.0),
        };
        let count = line.len() as f32;
        for (slot, player) in line.iter().enumerate() {
            let y = if position == Position::Goalkeeper {
                50.0
            } else {
                50.0 + span * ((slot as f32 + 0.5) / count - 0.5)
            };
            let x = match side {
                Side::Home => depth,
                Side::Away => 100.0 - depth,
            };
            dots.push(PlayerDot2D {
                player_id: player.id.clone(),
                side,
                position,
                x: clamp_on_pitch(x),
                y: y.clamp(EDGE, 100.0 - EDGE),
                has_ball: false,
            });
        }
    }
    dots
}

fn clamp_on_pitch(value: f32) -> f32 {
    value.clamp(EDGE, 100.0 - EDGE)
}

/// Move apart any two dots standing on the same spot, so every player can be seen.
fn separate(dots: &mut [PlayerDot2D]) {
    for later in 1..dots.len() {
        let mut nudge = 0;
        while dots[..later].iter().any(|earlier| {
            (earlier.x - dots[later].x).abs() < MIN_SEPARATION
                && (earlier.y - dots[later].y).abs() < MIN_SEPARATION
        }) && nudge < 50
        {
            nudge += 1;
            let step = if dots[later].y > 50.0 { -1.5 } else { 1.5 };
            dots[later].y = (dots[later].y + step).clamp(EDGE, 100.0 - EDGE);
            if dots[later].y <= EDGE || dots[later].y >= 100.0 - EDGE {
                dots[later].x = clamp_on_pitch(dots[later].x + 1.5);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::full_side;

    fn input<'a>(
        home: &'a TeamData,
        away: &'a TeamData,
        off: &'a HashSet<String>,
        zone: Zone,
        possession: Side,
    ) -> FrameInput<'a> {
        FrameInput {
            minute: 12,
            phase: "FirstHalf".to_string(),
            home,
            away,
            off_the_pitch: off,
            ball_zone: zone,
            possession,
            last_event: None,
        }
    }

    fn dot<'a>(frame: &'a PitchFrame2D, id: &str) -> &'a PlayerDot2D {
        frame
            .players
            .iter()
            .find(|dot| dot.player_id == id)
            .expect("on the pitch")
    }

    /// Given two full sides and the ball in every zone,
    /// When frames are drawn, Then every coordinate is on the pitch and no two players stand on
    /// the same spot.
    #[test]
    fn every_dot_is_on_the_pitch_and_none_is_stacked() {
        let (home, away, off) = (full_side("h"), full_side("a"), HashSet::new());
        for zone in [
            Zone::HomeBox,
            Zone::HomeDefense,
            Zone::Midfield,
            Zone::AwayDefense,
            Zone::AwayBox,
        ] {
            for possession in [Side::Home, Side::Away] {
                let frame = frame(&input(&home, &away, &off, zone, possession));
                assert_eq!(frame.players.len(), 22);
                for (index, a) in frame.players.iter().enumerate() {
                    assert!((0.0..=100.0).contains(&a.x) && (0.0..=100.0).contains(&a.y));
                    for b in &frame.players[index + 1..] {
                        assert!(
                            (a.x - b.x).abs() >= MIN_SEPARATION
                                || (a.y - b.y).abs() >= MIN_SEPARATION,
                            "{} and {} stacked at {zone:?}",
                            a.player_id,
                            b.player_id
                        );
                    }
                }
                assert!((0.0..=100.0).contains(&frame.ball.x));
            }
        }
    }

    /// Given a player sent off, When a frame is drawn, Then he is not in it.
    #[test]
    fn a_player_sent_off_is_not_drawn() {
        let (home, away) = (full_side("h"), full_side("a"));
        let off: HashSet<String> = ["h_def0".to_string()].into();
        let frame = frame(&input(&home, &away, &off, Zone::Midfield, Side::Home));
        assert_eq!(frame.players.len(), 21);
        assert!(frame.players.iter().all(|dot| dot.player_id != "h_def0"));
    }

    /// Given a side that defends high and one that sits deep,
    /// When frames are drawn with the ball at halfway, Then the high line stands further up.
    #[test]
    fn a_high_line_stands_further_up_than_a_deep_one() {
        let away = full_side("a");
        let off = HashSet::new();
        let mut high = full_side("h");
        high.tactics.defensive_line = DefensiveLine::High;
        let mut deep = full_side("h");
        deep.tactics.defensive_line = DefensiveLine::VeryLow;
        let high_frame = frame(&input(&high, &away, &off, Zone::Midfield, Side::Away));
        let deep_frame = frame(&input(&deep, &away, &off, Zone::Midfield, Side::Away));
        assert!(dot(&high_frame, "h_def0").x > dot(&deep_frame, "h_def0").x);
    }

    /// Given a side that plays wide and one that plays narrow,
    /// When frames are drawn, Then the wide side's back line spans more of the pitch.
    #[test]
    fn a_wide_side_spreads_further_across() {
        let away = full_side("a");
        let off = HashSet::new();
        let span = |width: TacticsPitchWidth| {
            let mut home = full_side("h");
            home.tactics.width = width;
            let frame = frame(&input(&home, &away, &off, Zone::Midfield, Side::Home));
            dot(&frame, "h_def3").y - dot(&frame, "h_def0").y
        };
        assert!(span(TacticsPitchWidth::Wide) > span(TacticsPitchWidth::Narrow));
    }

    /// Given the ball in the away box with the home side on it,
    /// When a frame is drawn, Then the home side's lines stand further up than with the ball in
    /// its own box, and the away side mirrors the home side's shape.
    #[test]
    fn the_shape_follows_the_ball() {
        let (home, away, off) = (full_side("h"), full_side("a"), HashSet::new());
        let attacking = frame(&input(&home, &away, &off, Zone::AwayBox, Side::Home));
        let defending = frame(&input(&home, &away, &off, Zone::HomeBox, Side::Home));
        assert!(dot(&attacking, "h_mid0").x > dot(&defending, "h_mid0").x);
        assert!(dot(&attacking, "a_gk").x > 90.0 && dot(&attacking, "h_gk").x < 15.0);
    }

    /// Given the last event a home player's,
    /// When a frame is drawn, Then he has the ball and the ball is at his feet.
    #[test]
    fn the_player_who_acted_last_has_the_ball() {
        let (home, away, off) = (full_side("h"), full_side("a"), HashSet::new());
        let event = MatchEvent::new(12, EventType::Dribble, Side::Home, Zone::AwayDefense)
            .with_player("h_fwd1");
        let mut frame_input = input(&home, &away, &off, Zone::AwayDefense, Side::Home);
        frame_input.last_event = Some(&event);
        let frame = frame(&frame_input);
        let carrier = dot(&frame, "h_fwd1");
        assert!(carrier.has_ball);
        assert_eq!(frame.ball.owner_id.as_deref(), Some("h_fwd1"));
        assert!((frame.ball.x - carrier.x).abs() <= 1.5);
    }

    /// Given a match's events with a sending-off,
    /// When its frames are rebuilt, Then there is one a minute from the kick-off, and the
    /// player sent off is gone from the minute of his card.
    #[test]
    fn frames_rebuilt_from_events_drop_a_player_sent_off() {
        let (home, away) = (full_side("h"), full_side("a"));
        let events = vec![
            MatchEvent::new(0, EventType::KickOff, Side::Home, Zone::Midfield),
            MatchEvent::new(3, EventType::RedCard, Side::Away, Zone::Midfield)
                .with_player("a_mid1"),
            MatchEvent::new(5, EventType::FullTime, Side::Home, Zone::Midfield),
        ];
        let frames = frames_from_events(&home, &away, &events);
        assert_eq!(frames.len(), 6);
        assert_eq!(frames[2].players.len(), 22);
        assert_eq!(frames[3].players.len(), 21);
    }

    /// Given events followed by a penalty shootout at higher minute values,
    /// When frames are rebuilt, Then emission stops once shootout kicks begin.
    #[test]
    fn frames_rebuilt_from_events_stop_when_shootout_starts() {
        let (home, away) = (full_side("h"), full_side("a"));
        let events = vec![
            MatchEvent::new(0, EventType::KickOff, Side::Home, Zone::Midfield),
            MatchEvent::new(2, EventType::HalfTime, Side::Home, Zone::Midfield),
            MatchEvent::new(5, EventType::ShootoutGoal, Side::Home, Zone::AwayBox)
                .with_player("h_fwd0"),
        ];
        let frames = frames_from_events(&home, &away, &events);
        assert_eq!(frames.len(), 5);
    }
}
