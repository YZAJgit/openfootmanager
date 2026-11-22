//! Expected goals and the shot map.
//!
//! The engine resolves a shot from the shooter's and the keeper's attributes; it has no ball
//! position finer than a zone. The shot map gives each shot a place on the pitch, derived
//! deterministically from the shot itself (who took it, when, how) and never from the match's
//! random stream, so recording shots cannot change a single result. Its expected-goals value is a
//! pure function of that place and the body part.
//!
//! Coordinates are the shared normalised pitch (`pitch_2d`): `x` 0 to 100 from the home goal to
//! the away goal, `y` 0 to 100 across. The home side attacks `x = 100`, the away side `x = 0`.

use serde::{Deserialize, Serialize};

use crate::types::Side;

/// What became of a shot.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, ts_rs::TS)]
#[ts(rename_all = "PascalCase")]
pub enum ShotOutcome {
    Goal,
    Saved,
    Blocked,
    OffTarget,
    /// Reserved for a consumer that tells woodwork apart: the engine does not model the posts
    /// yet, so it records a shot that missed as `OffTarget`.
    Woodwork,
}

/// How the shot was struck.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, ts_rs::TS)]
#[ts(rename_all = "PascalCase")]
pub enum ShotBodyPart {
    Foot,
    Header,
    /// A direct free kick.
    SetPiece,
    Penalty,
}

/// One shot on the shot map. Shootout kicks are not shots of the match and are never recorded.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, ts_rs::TS)]
pub struct ShotMapEntry {
    pub minute: u8,
    pub side: Side,
    pub shooter_id: String,
    pub x: f32,
    pub y: f32,
    pub xg: f32,
    pub outcome: ShotOutcome,
    pub body_part: ShotBodyPart,
    pub assisted_by: Option<String>,
}

/// Pitch length and width in metres, for turning the normalised pitch into distances.
const PITCH_LENGTH_M: f32 = 105.0;
const PITCH_WIDTH_M: f32 = 68.0;
const HALF_GOAL_WIDTH_M: f32 = 3.66;
const PENALTY_SPOT_M: f32 = 11.0;

/// The value of a penalty: about the share of penalties scored in the top European leagues.
const PENALTY_XG: f32 = 0.78;

/// Expected goals for a shot from (`x`, `y`) by `attacking_side` struck with `body_part`, in
/// `0.01..=0.95`. A logistic in distance to the goal's centre and the angle the goal mouth
/// presents, scaled so a match's xG adds up to about the goals this engine scores from open play
/// (about 0.35 from six metres, 0.12 from the penalty spot, 0.05 from the edge of the box, 0.02
/// from thirty metres): close and central is worth more than far or wide, a header less than a
/// shot with the foot from the same place, and a penalty its own fixed value.
pub fn compute_shot_xg(x: f32, y: f32, attacking_side: Side, body_part: ShotBodyPart) -> f32 {
    if body_part == ShotBodyPart::Penalty {
        return PENALTY_XG;
    }
    if !x.is_finite() || !y.is_finite() {
        return 0.01;
    }
    let goal_x = match attacking_side {
        Side::Home => 100.0,
        Side::Away => 0.0,
    };
    let along = ((goal_x - x.clamp(0.0, 100.0)).abs() / 100.0 * PITCH_LENGTH_M).max(0.5);
    let across = (y.clamp(0.0, 100.0) - 50.0) / 100.0 * PITCH_WIDTH_M;
    let distance = (along * along + across * across).sqrt();
    // The angle between the lines to the two posts.
    let mut angle =
        (2.0 * HALF_GOAL_WIDTH_M * along).atan2(distance * distance - HALF_GOAL_WIDTH_M.powi(2));
    if angle < 0.0 {
        angle += std::f32::consts::PI;
    }
    let body = match body_part {
        ShotBodyPart::Foot => 0.0,
        ShotBodyPart::Header => -1.0,
        // A direct free kick: a wall in the way, but a dead ball and a specialist striking it.
        ShotBodyPart::SetPiece => 0.0,
        ShotBodyPart::Penalty => 0.0,
    };
    let logit = -2.55 - 0.07 * distance + 2.2 * angle + body;
    (1.0 / (1.0 + (-logit).exp())).clamp(0.01, 0.95)
}

/// Where a shot was taken from, for the shot map: a place typical of the body part (a shot with
/// the foot from 7 to 30 metres, about 18 on average as in the top leagues; a header from inside
/// the box), closer to goal for a sharper shooter (`sharpness` 0 to 1), spread by `key`, a hash of
/// the shot itself. The same shot always lands in the same place.
pub(crate) fn shot_location(
    attacking_side: Side,
    body_part: ShotBodyPart,
    sharpness: f32,
    key: u64,
) -> (f32, f32) {
    let (along_roll, across_roll) = unit_pair(key);
    let sharpness = sharpness.clamp(0.0, 1.0);
    let (distance, max_offset) = match body_part {
        ShotBodyPart::Foot => {
            let distance = 7.0 + 23.0 * along_roll.powf(0.6 + 0.6 * sharpness);
            (distance, (distance * 0.8).min(20.0))
        }
        ShotBodyPart::Header => {
            let distance = 5.0 + 7.0 * along_roll;
            (distance, (distance * 0.6).min(8.0))
        }
        ShotBodyPart::SetPiece => (18.0 + 10.0 * along_roll, 12.0),
        ShotBodyPart::Penalty => (PENALTY_SPOT_M, 0.0),
    };
    let offset = (across_roll - 0.5) * 2.0 * max_offset;
    let along = (distance * distance - offset * offset).max(1.0).sqrt();
    let depth = along / PITCH_LENGTH_M * 100.0;
    let x = match attacking_side {
        Side::Home => 100.0 - depth,
        Side::Away => depth,
    };
    let y = 50.0 + offset / PITCH_WIDTH_M * 100.0;
    (x.clamp(0.0, 100.0), y.clamp(0.0, 100.0))
}

/// A stable hash of a shot: its minute, its shooter and how many shots came before it.
pub(crate) fn shot_key(minute: u8, shooter_id: &str, shots_before: usize) -> u64 {
    // FNV-1a: stable across platforms and releases, unlike `std`'s hasher.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let bytes = shooter_id
        .bytes()
        .chain([minute])
        .chain((shots_before as u64).to_le_bytes());
    for byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Two numbers in `0..1` from one hash.
fn unit_pair(key: u64) -> (f32, f32) {
    let mixed = key ^ (key >> 29);
    let first = (mixed & 0xffff) as f32 / 65_535.0;
    let second = ((mixed >> 16) & 0xffff) as f32 / 65_535.0;
    (first, second)
}

/// One shot as the engines record it, before it is placed on the pitch.
pub(crate) struct ShotRecord<'a> {
    pub minute: u8,
    pub side: Side,
    pub shooter_id: &'a str,
    pub body_part: ShotBodyPart,
    pub outcome: ShotOutcome,
    pub assisted_by: Option<String>,
    /// How sharp the shooter is, 0 to 1: his shooting rating out of 100.
    pub sharpness: f32,
}

impl ShotRecord<'_> {
    /// The shot placed and valued, as the `shots_before`-th shot of its match.
    pub(crate) fn into_entry(self, shots_before: usize) -> ShotMapEntry {
        let key = shot_key(self.minute, self.shooter_id, shots_before);
        let (x, y) = shot_location(self.side, self.body_part, self.sharpness, key);
        let assisted_by = self.assisted_by.filter(|id| id != self.shooter_id);
        ShotMapEntry {
            minute: self.minute,
            side: self.side,
            shooter_id: self.shooter_id.to_string(),
            x,
            y,
            xg: compute_shot_xg(x, y, self.side, self.body_part),
            outcome: self.outcome,
            body_part: self.body_part,
            assisted_by,
        }
    }
}

/// The sum of a side's expected goals.
pub(crate) fn total_xg<'a>(shots: impl Iterator<Item = &'a ShotMapEntry>, side: Side) -> f32 {
    shots
        .filter(|shot| shot.side == side)
        .map(|shot| shot.xg)
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Given a shot from the six-yard box, from the penalty spot and from thirty metres, all central,
    /// When their expected goals are computed,
    /// Then the closer shot is always worth more.
    #[test]
    fn a_closer_central_shot_is_worth_more() {
        let at = |metres: f32| {
            compute_shot_xg(100.0 - metres / 1.05, 50.0, Side::Home, ShotBodyPart::Foot)
        };
        assert!(at(6.0) > at(11.0));
        assert!(at(11.0) > at(30.0));
    }

    /// Given two shots the same distance along the pitch, one central and one from the wing,
    /// When valued, Then the central one is worth more.
    #[test]
    fn a_central_shot_is_worth_more_than_a_wide_one() {
        let central = compute_shot_xg(88.0, 50.0, Side::Home, ShotBodyPart::Foot);
        let wide = compute_shot_xg(88.0, 80.0, Side::Home, ShotBodyPart::Foot);
        assert!(central > wide, "{central} vs {wide}");
    }

    /// Given a header and a shot with the foot from the same place,
    /// When valued, Then the header is worth less.
    #[test]
    fn a_header_is_worth_less_than_a_shot_with_the_foot_from_the_same_place() {
        let foot = compute_shot_xg(92.0, 50.0, Side::Home, ShotBodyPart::Foot);
        let header = compute_shot_xg(92.0, 50.0, Side::Home, ShotBodyPart::Header);
        assert!(header < foot);
    }

    /// Given a penalty, When valued, Then it is about the share of penalties scored.
    #[test]
    fn a_penalty_is_worth_about_what_penalties_return() {
        let penalty = compute_shot_xg(89.5, 50.0, Side::Away, ShotBodyPart::Penalty);
        assert!((0.76..=0.79).contains(&penalty));
    }

    /// Given the away side shooting at the home goal,
    /// When the mirror of a home shot is valued,
    /// Then it is worth the same: the model has no favourite end.
    #[test]
    fn the_two_ends_are_mirror_images() {
        let home = compute_shot_xg(90.0, 40.0, Side::Home, ShotBodyPart::Foot);
        let away = compute_shot_xg(10.0, 40.0, Side::Away, ShotBodyPart::Foot);
        assert!((home - away).abs() < 1e-6);
    }

    /// Given every place on the pitch and every body part,
    /// When valued, Then the value is within 0.01 and 0.95.
    #[test]
    fn every_value_is_bounded() {
        for x in (0..=100).step_by(5) {
            for y in (0..=100).step_by(5) {
                for body in [
                    ShotBodyPart::Foot,
                    ShotBodyPart::Header,
                    ShotBodyPart::SetPiece,
                ] {
                    for side in [Side::Home, Side::Away] {
                        let xg = compute_shot_xg(x as f32, y as f32, side, body);
                        assert!(
                            (0.01..=0.95).contains(&xg),
                            "{x},{y} {body:?} {side:?}: {xg}"
                        );
                    }
                }
            }
        }
    }

    /// Given the same shot placed twice, and many different shots,
    /// When located, Then the same shot lands in the same place, and every place is on the
    /// attacking half of the pitch.
    #[test]
    fn a_shot_is_placed_the_same_way_every_time_and_on_the_pitch() {
        let key = shot_key(37, "h_fwd0", 4);
        assert_eq!(
            shot_location(Side::Home, ShotBodyPart::Foot, 0.7, key),
            shot_location(Side::Home, ShotBodyPart::Foot, 0.7, key)
        );
        for index in 0..500 {
            let key = shot_key((index % 120) as u8, "a_9", index);
            for body in [
                ShotBodyPart::Foot,
                ShotBodyPart::Header,
                ShotBodyPart::SetPiece,
                ShotBodyPart::Penalty,
            ] {
                let (x, y) = shot_location(Side::Away, body, 0.5, key);
                assert!((0.0..=50.0).contains(&x), "{body:?}: x {x}");
                assert!((0.0..=100.0).contains(&y), "{body:?}: y {y}");
            }
        }
    }

    /// Given a shot recorded with the shooter as his own assister,
    /// When it is placed on the map, Then it has no assister.
    #[test]
    fn a_shooter_does_not_assist_himself() {
        let entry = ShotRecord {
            minute: 10,
            side: Side::Home,
            shooter_id: "h_9",
            body_part: ShotBodyPart::Foot,
            outcome: ShotOutcome::Saved,
            assisted_by: Some("h_9".to_string()),
            sharpness: 0.7,
        }
        .into_entry(0);
        assert_eq!(entry.assisted_by, None);
    }

    /// Given non-finite coordinates supplied to the public xG helper,
    /// When valued, Then it clamps to the floor instead of propagating NaN.
    #[test]
    fn non_finite_coordinates_do_not_propagate_nan() {
        assert_eq!(
            compute_shot_xg(f32::NAN, 50.0, Side::Home, ShotBodyPart::Foot),
            0.01
        );
        assert_eq!(
            compute_shot_xg(90.0, f32::INFINITY, Side::Away, ShotBodyPart::Header),
            0.01
        );
    }
}
