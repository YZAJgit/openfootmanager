//! The instant engine's side of `extras`: who leads each side on the pitch, what its team talk
//! is doing, its set-piece takers at work, and the shot map. The rules are `extras`'s, shared
//! with the live match.

use rand::{Rng, RngExt};

use crate::event::{EventType, MatchEvent};
use crate::extras::{
    FREE_KICK_IN_SHOOTING_RANGE, PendingSetPiece, TalkEffect, TeamExtras, acting_captain,
    captain_composure, captain_discipline, delivery_quality, free_kick_accuracy,
    free_kick_conversion, free_kick_delivery_chance, free_kick_skill, shoots_free_kicks,
    taker_on_the_pitch, team_talk_effect, under_pressure,
};
use crate::live_match::SetPieceTakers;
use crate::shared::{PlayerSnap, TraitContext, keeper_role_factor, trait_bonus};
use crate::types::{PlayerData, Position, Side, Zone};
use crate::xg::{ShotBodyPart, ShotOutcome, ShotRecord};

use super::{MatchContext, snap_player};

impl MatchContext<'_> {
    fn extras(&self, side: Side) -> &TeamExtras {
        match side {
            Side::Home => self.home_extras,
            Side::Away => self.away_extras,
        }
    }

    fn on_the_pitch(&self, id: &str) -> bool {
        !self.sent_off.contains(id)
    }

    fn acting_captain(&self, side: Side) -> Option<&PlayerData> {
        acting_captain(
            &self.team(side).players,
            &self.extras(side).set_pieces,
            |id| self.on_the_pitch(id),
        )
    }

    fn goal_difference(&self, side: Side) -> i16 {
        let (home, away) = (i16::from(self.home_score), i16::from(self.away_score));
        match side {
            Side::Home => home - away,
            Side::Away => away - home,
        }
    }

    /// What `side`'s team talk, given at kick-off, is doing to it this minute.
    pub(super) fn talk_effect(&self, side: Side) -> TalkEffect {
        team_talk_effect(
            self.extras(side).talk,
            self.goal_difference(side),
            self.minute,
        )
    }

    /// The factor on `side`'s players' ratings for its talk and its captain: exactly 1.0 for a
    /// side told nothing and with no captain on the pitch.
    pub(super) fn mentality(&self, side: Side) -> f64 {
        let leadership = self.acting_captain(side).map(|captain| captain.leadership);
        let pressure = under_pressure(side, self.goal_difference(side));
        self.talk_effect(side).morale * captain_composure(leadership, pressure)
    }

    /// The factor on `side`'s fouls: its captain's hold on it and its talk's fire.
    pub(super) fn foul_temper(&self, side: Side) -> f64 {
        self.card_temper(side) * self.talk_effect(side).aggression
    }

    /// The factor on the cards `side`'s fouls earn.
    pub(super) fn card_temper(&self, side: Side) -> f64 {
        captain_discipline(self.acting_captain(side).map(|captain| captain.leadership))
    }

    fn designated_taker(
        &self,
        side: Side,
        designated: impl Fn(&SetPieceTakers) -> Option<&String>,
    ) -> Option<PlayerSnap> {
        taker_on_the_pitch(
            &self.team(side).players,
            designated(&self.extras(side).set_pieces).map(String::as_str),
            |id| self.on_the_pitch(id),
        )
    }

    /// `side`'s designated penalty taker, while he is on the pitch; `None` leaves the pick to
    /// the side's own default, as the live engine does.
    pub(super) fn penalty_taker(&self, side: Side) -> Option<PlayerSnap> {
        self.designated_taker(side, |takers| takers.penalty_taker.as_ref())
    }

    /// A corner for `side` played into the box: who delivered it and how well.
    pub(super) fn corner_delivery(&self, side: Side) -> PendingSetPiece {
        let taker = self.designated_taker(side, |takers| takers.corner_taker.as_ref());
        PendingSetPiece {
            side,
            quality: taker.as_ref().map_or(1.0, delivery_quality),
            taker_id: taker.map(|taker| taker.id),
        }
    }

    /// The rating a keeper of `side` faces a shot with.
    pub(super) fn keeper_rating(&self, keeper: &PlayerSnap, side: Side) -> f64 {
        (keeper.handling as f64 + keeper.reflexes as f64 + keeper.positioning as f64) / 3.0
            * trait_bonus(keeper, TraitContext::Goalkeeping)
            * self.condition(side)
            * keeper_role_factor(keeper.role)
            * self.mentality(side)
    }

    pub(super) fn condition(&self, side: Side) -> f64 {
        match side {
            Side::Home => self.home_condition,
            Side::Away => self.away_condition,
        }
    }

    pub(super) fn record_shot(&mut self, shot: ShotRecord<'_>) {
        let entry = shot.into_entry(self.shots.len());
        self.shots.push(entry);
    }

    /// The free kick `att_side` has just won in the attacking third, taken by its designated
    /// taker; with none on the pitch, nothing happens here.
    pub(super) fn take_free_kick<R: Rng>(&mut self, minute: u8, att_side: Side, rng: &mut R) {
        let Some(taker) = self.designated_taker(att_side, |takers| takers.free_kick_taker.as_ref())
        else {
            return;
        };
        if shoots_free_kicks(&taker) && rng.random_range(0.0..1.0f64) < FREE_KICK_IN_SHOOTING_RANGE
        {
            self.direct_free_kick(minute, att_side, &taker, rng);
            return;
        }
        let quality = delivery_quality(&taker);
        if rng.random_range(0.0..1.0f64) < free_kick_delivery_chance(quality) {
            self.ball_zone = Zone::attacking_box(att_side);
            self.pending_set_piece = Some(PendingSetPiece {
                side: att_side,
                taker_id: Some(taker.id),
                quality,
            });
        } else {
            let def_side = att_side.opposite();
            let defender = snap_player(self, def_side, Position::Defender, rng);
            let zone = Zone::attacking_third(att_side);
            self.emit(
                MatchEvent::new(minute, EventType::Clearance, def_side, zone)
                    .with_player(&defender.id),
            );
            self.possession = def_side;
            self.ball_zone = Zone::defensive_third(att_side);
        }
    }

    fn direct_free_kick<R: Rng>(
        &mut self,
        minute: u8,
        att_side: Side,
        taker: &PlayerSnap,
        rng: &mut R,
    ) {
        let def_side = att_side.opposite();
        let zone = Zone::attacking_third(att_side);
        let skill = free_kick_skill(taker) * self.condition(att_side) * self.mentality(att_side);
        let keeper = snap_player(self, def_side, Position::Goalkeeper, rng);
        let keeper_rating = self.keeper_rating(&keeper, def_side);
        let shot = |outcome| ShotRecord {
            minute,
            side: att_side,
            shooter_id: &taker.id,
            body_part: ShotBodyPart::SetPiece,
            outcome,
            assisted_by: None,
            sharpness: (skill / 100.0) as f32,
        };

        if rng.random_range(0.0..1.0f64) >= free_kick_accuracy(skill) {
            self.emit(
                MatchEvent::new(minute, EventType::ShotOffTarget, att_side, zone)
                    .with_player(&taker.id),
            );
            self.emit(MatchEvent::new(minute, EventType::GoalKick, def_side, zone));
            self.record_shot(shot(ShotOutcome::OffTarget));
            self.possession = def_side;
            self.ball_zone = Zone::defensive_third(def_side);
        } else if rng.random_range(0.0..1.0f64) < free_kick_conversion(skill, keeper_rating) {
            self.emit(
                MatchEvent::new(minute, EventType::Goal, att_side, zone).with_player(&taker.id),
            );
            self.record_shot(shot(ShotOutcome::Goal));
            self.add_goal(att_side);
            self.possession = def_side;
            self.ball_zone = Zone::Midfield;
        } else {
            self.emit(
                MatchEvent::new(minute, EventType::ShotSaved, att_side, zone)
                    .with_player(&taker.id),
            );
            self.emit(MatchEvent::new(minute, EventType::GoalKick, def_side, zone));
            self.record_shot(shot(ShotOutcome::Saved));
            self.possession = def_side;
            self.ball_zone = Zone::defensive_third(def_side);
        }
    }
}
