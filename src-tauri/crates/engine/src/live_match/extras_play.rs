//! The live match's side of `extras`: who leads each side on the pitch, what its team talk is
//! doing, its set-piece takers at work, and the shot map.

use rand::{Rng, RngExt};

use crate::event::{EventDetail, EventType, MatchEvent};
use crate::extras::{
    FREE_KICK_IN_SHOOTING_RANGE, PendingSetPiece, TalkEffect, acting_captain, captain_composure,
    captain_discipline, delivery_quality, free_kick_accuracy, free_kick_conversion,
    free_kick_delivery_chance, free_kick_skill, shoots_free_kicks, taker_on_the_pitch,
    team_talk_effect, under_pressure,
};
use crate::shared::{PlayerSnap, TraitContext, keeper_role_factor, trait_bonus};
use crate::types::{PlayerData, Position, Side, Zone};
use crate::xg::{ShotBodyPart, ShotOutcome, ShotRecord};

use super::LiveMatchState;
use super::SetPieceTakers;
use super::helpers::save_quality;

impl LiveMatchState {
    /// The player leading `side` on the pitch: its captain, else its vice-captain.
    pub(super) fn acting_captain(&self, side: Side) -> Option<&PlayerData> {
        acting_captain(
            &self.team_ref(side).players,
            self.set_pieces_ref(side),
            |id| self.is_on_the_pitch(side, id),
        )
    }

    /// `side`'s goals less its opponent's.
    fn goal_difference(&self, side: Side) -> i16 {
        let (own, against) = self.score_for(side);
        i16::from(own) - i16::from(against)
    }

    /// What `side`'s team talk is doing to it this minute.
    pub(super) fn talk_effect(&self, side: Side) -> TalkEffect {
        let (tone, given_at) = match side {
            Side::Home => self.home_talk,
            Side::Away => self.away_talk,
        };
        team_talk_effect(
            tone,
            self.goal_difference(side),
            self.current_minute.saturating_sub(given_at),
        )
    }

    /// The factor on `side`'s players' ratings for its talk and its captain: exactly 1.0 for a
    /// side told nothing and with no captain on the pitch.
    pub(super) fn side_mentality(&self, side: Side) -> f64 {
        let leadership = self.acting_captain(side).map(|captain| captain.leadership);
        let pressure = under_pressure(side, self.goal_difference(side));
        self.talk_effect(side).morale * captain_composure(leadership, pressure)
    }

    /// The factor on `side`'s fouls: its captain's hold on it and its talk's fire.
    pub(super) fn foul_temper(&self, side: Side) -> f64 {
        self.card_temper(side) * self.talk_effect(side).aggression
    }

    /// The factor on the cards `side`'s fouls earn: a strong captain talks his man out of one.
    pub(super) fn card_temper(&self, side: Side) -> f64 {
        captain_discipline(self.acting_captain(side).map(|captain| captain.leadership))
    }

    /// `side`'s taker for a set piece `designated` names, while he is on the pitch.
    fn designated_taker(
        &self,
        side: Side,
        designated: impl Fn(&SetPieceTakers) -> Option<&String>,
    ) -> Option<PlayerSnap> {
        taker_on_the_pitch(
            &self.team_ref(side).players,
            designated(self.set_pieces_ref(side)).map(String::as_str),
            |id| self.is_on_the_pitch(side, id),
        )
        .map(|mut snap| {
            snap.morale_factor *= self.side_mentality(side);
            snap
        })
    }

    /// A corner for `side` played into the box: who delivered it and how well. With no corner
    /// taker on the pitch it is nobody's, at the plain chance (1.0).
    pub(super) fn corner_delivery(&self, side: Side) -> PendingSetPiece {
        let taker = self.designated_taker(side, |takers| takers.corner_taker.as_ref());
        PendingSetPiece {
            side,
            quality: taker.as_ref().map_or(1.0, delivery_quality),
            taker_id: taker.map(|taker| taker.id),
        }
    }

    /// The rating a keeper faces a shot with.
    pub(super) fn keeper_rating(&self, keeper: &PlayerSnap) -> f64 {
        let raw =
            (keeper.handling as f64 + keeper.reflexes as f64 + keeper.positioning as f64) / 3.0;
        self.condition_adjusted_skill(keeper, raw)
            * trait_bonus(keeper, TraitContext::Goalkeeping)
            * keeper_role_factor(keeper.role)
    }

    /// Put a shot on the map.
    pub(super) fn record_shot(&mut self, shot: ShotRecord<'_>) {
        let entry = shot.into_entry(self.shots.len());
        self.shots.push(entry);
    }

    fn log(&mut self, events: &mut Vec<MatchEvent>, event: MatchEvent) {
        self.events.push(event.clone());
        events.push(event);
    }

    /// The free kick `att_side` has just won in the attacking third, taken by its designated
    /// taker: he goes for goal if he is a striker of the ball, and puts it into the box if he is a
    /// passer. With no free-kick taker on the pitch nothing happens here, and the side plays on
    /// from the free kick as it always has.
    pub(super) fn take_free_kick<R: Rng>(
        &mut self,
        minute: u8,
        att_side: Side,
        rng: &mut R,
    ) -> Vec<MatchEvent> {
        let Some(taker) = self.designated_taker(att_side, |takers| takers.free_kick_taker.as_ref())
        else {
            return Vec::new();
        };
        if shoots_free_kicks(&taker) && rng.random_range(0.0..1.0f64) < FREE_KICK_IN_SHOOTING_RANGE
        {
            return self.direct_free_kick(minute, att_side, &taker, rng);
        }
        let mut events = Vec::new();
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
            let defender = self.snap_player(def_side, Position::Defender, rng);
            let zone = Zone::attacking_third(att_side);
            let clearance = MatchEvent::new(minute, EventType::Clearance, def_side, zone)
                .with_player(&defender.id);
            self.log(&mut events, clearance);
            self.possession = def_side;
            self.ball_zone = Zone::defensive_third(att_side);
        }
        events
    }

    /// A free kick struck at goal.
    fn direct_free_kick<R: Rng>(
        &mut self,
        minute: u8,
        att_side: Side,
        taker: &PlayerSnap,
        rng: &mut R,
    ) -> Vec<MatchEvent> {
        let mut events = Vec::new();
        let def_side = att_side.opposite();
        let zone = Zone::attacking_third(att_side);
        let skill = self.condition_adjusted_skill(taker, free_kick_skill(taker));
        let keeper = self.pick_goalkeeper(def_side);
        let keeper_rating = self.keeper_rating(&keeper);
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
            let off = MatchEvent::new(minute, EventType::ShotOffTarget, att_side, zone)
                .with_player(&taker.id);
            self.log(&mut events, off);
            self.log(
                &mut events,
                MatchEvent::new(minute, EventType::GoalKick, def_side, zone),
            );
            self.record_shot(shot(ShotOutcome::OffTarget));
            self.possession = def_side;
            self.ball_zone = Zone::defensive_third(def_side);
        } else if rng.random_range(0.0..1.0f64) < free_kick_conversion(skill, keeper_rating) {
            let context = self.goal_context(att_side);
            let goal = MatchEvent::new(minute, EventType::Goal, att_side, zone)
                .with_player(&taker.id)
                .with_detail(EventDetail::Goal { context });
            self.log(&mut events, goal);
            self.record_shot(shot(ShotOutcome::Goal));
            self.add_goal(att_side);
            self.possession = def_side;
            self.ball_zone = Zone::Midfield;
        } else {
            let saved = MatchEvent::new(minute, EventType::ShotSaved, att_side, zone)
                .with_player(&taker.id)
                .with_detail(EventDetail::Save {
                    quality: save_quality(keeper_rating),
                });
            self.log(&mut events, saved);
            self.log(
                &mut events,
                MatchEvent::new(minute, EventType::GoalKick, def_side, zone),
            );
            self.record_shot(shot(ShotOutcome::Saved));
            self.possession = def_side;
            self.ball_zone = Zone::defensive_third(def_side);
        }
        events
    }
}
