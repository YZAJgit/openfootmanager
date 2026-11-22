use rand::{Rng, RngExt};

use crate::event::{EventDetail, EventType, MatchEvent};
use crate::extras::{CORNER_INTO_THE_BOX, ShotOrigin};
use crate::shared::{
    PlayStylePhase, PlayerSnap, TraitContext, assist_creation_factor, keeper_buildup_factor,
    line_role_factor, play_style_modifier, role_modifier, tactics_buildup_mod,
    tactics_cross_probability, tactics_defensive_conversion_mod, tactics_foul_modifier,
    tactics_shape_modifier, tactics_tempo_progression, tactics_width_versus_shape, trait_bonus,
};
use crate::types::{Position, Side, Zone};
use crate::xg::{ShotOutcome, ShotRecord};

use super::LiveMatchState;
use super::helpers::{danger_band, foul_severity, save_quality};

// ---------------------------------------------------------------------------
// Action resolution
// ---------------------------------------------------------------------------

impl LiveMatchState {
    pub(super) fn resolve_action<R: Rng>(&mut self, minute: u8, rng: &mut R) -> Vec<MatchEvent> {
        let att_side = self.possession;
        let def_side = att_side.opposite();
        let zone = self.ball_zone;

        // A set piece put into the box is headed at goal by the next action there, or is lost.
        let pending = self.pending_set_piece.take();
        if zone.is_box_for(att_side) {
            self.resolve_shot(
                minute,
                att_side,
                ShotOrigin::in_the_box(pending, att_side),
                rng,
            )
        } else if zone == Zone::attacking_third(att_side) {
            self.resolve_attacking_third(minute, att_side, def_side, rng)
        } else if zone == Zone::Midfield {
            self.resolve_midfield(minute, att_side, def_side, rng)
        } else {
            self.resolve_buildup(minute, att_side, def_side, rng)
        }
    }

    fn resolve_buildup<R: Rng>(
        &mut self,
        minute: u8,
        att_side: Side,
        def_side: Side,
        rng: &mut R,
    ) -> Vec<MatchEvent> {
        let mut events = Vec::new();
        let passer = self.snap_player(att_side, Position::Defender, rng);
        let pass_skill = self.condition_adjusted_skill(
            &passer,
            (passer.passing as f64
                + passer.vision as f64
                + passer.composure as f64
                + passer.teamwork as f64)
                / 4.0,
        ) * trait_bonus(&passer, TraitContext::Passing)
            * role_modifier(Position::Defender, passer.role, PlayStylePhase::Midfield)
            * keeper_buildup_factor(self.goalkeeper_in_goal(att_side).map(|keeper| keeper.role));
        let press = self.effective_press(def_side);
        let ball_zone = self.ball_zone;

        let buildup_mod = tactics_buildup_mod(&self.team_ref(att_side).tactics);
        let success_chance =
            (pass_skill * 1.3 * buildup_mod) / (pass_skill * 1.3 * buildup_mod + press);
        if rng.random_range(0.0..1.0f64) < success_chance {
            let evt = MatchEvent::new(minute, EventType::PassCompleted, att_side, ball_zone)
                .with_player(&passer.id);
            self.events.push(evt.clone());
            events.push(evt);
            self.ball_zone = Zone::Midfield;
        } else {
            let interceptor = self.snap_player(def_side, Position::Midfielder, rng);
            let evt1 = MatchEvent::new(minute, EventType::PassIntercepted, att_side, ball_zone)
                .with_player(&passer.id);
            let evt2 = MatchEvent::new(minute, EventType::Interception, def_side, ball_zone)
                .with_player(&interceptor.id);
            self.events.push(evt1.clone());
            self.events.push(evt2.clone());
            events.push(evt1);
            events.push(evt2);
            self.possession = def_side;
        }
        events
    }

    fn resolve_midfield<R: Rng>(
        &mut self,
        minute: u8,
        att_side: Side,
        def_side: Side,
        rng: &mut R,
    ) -> Vec<MatchEvent> {
        let mut events = Vec::new();
        let attacker = self.snap_player(att_side, Position::Midfielder, rng);
        let defender = self.snap_player(def_side, Position::Midfielder, rng);

        let att_raw = (attacker.dribbling as f64
            + attacker.passing as f64
            + attacker.vision as f64
            + attacker.teamwork as f64)
            / 4.0;
        let def_raw = (defender.tackling as f64
            + defender.positioning as f64
            + defender.decisions as f64
            + defender.teamwork as f64)
            / 4.0;
        let att_rating = self.condition_adjusted_skill(&attacker, att_raw)
            * trait_bonus(&attacker, TraitContext::Midfield);
        let def_rating = self.condition_adjusted_skill(&defender, def_raw)
            * trait_bonus(&defender, TraitContext::Tackling)
            * self.short_handed_cover(def_side);

        let att_mod = play_style_modifier(
            self.team_ref(att_side).play_style,
            PlayStylePhase::Midfield,
            true,
        ) * role_modifier(
            Position::Midfielder,
            attacker.role,
            PlayStylePhase::Midfield,
        );
        let def_mod =
            play_style_modifier(
                self.team_ref(def_side).play_style,
                PlayStylePhase::Midfield,
                false,
            ) * role_modifier(Position::Midfielder, defender.role, PlayStylePhase::Defense);
        let att_eff = att_rating
            * att_mod
            * crate::shared::home_mod(att_side, &self.config)
            * tactics_tempo_progression(&self.team_ref(att_side).tactics)
            * self.line_on_the_pitch(att_side, Position::Forward, PlayStylePhase::Midfield);
        let def_eff = def_rating * def_mod * crate::shared::home_mod(def_side, &self.config);
        let success = att_eff / (att_eff + def_eff);

        if rng.random_range(0.0..1.0f64) < success {
            let evt = MatchEvent::new(minute, EventType::PassCompleted, att_side, Zone::Midfield)
                .with_player(&attacker.id);
            self.events.push(evt.clone());
            events.push(evt);
            self.ball_zone = Zone::attacking_third(att_side);
        } else {
            if rng.random_range(0.0..1.0f64) < 0.6 {
                let evt = MatchEvent::new(minute, EventType::Tackle, def_side, Zone::Midfield)
                    .with_player(&defender.id);
                self.events.push(evt.clone());
                events.push(evt);
                let foul_mod = tactics_foul_modifier(&self.team_ref(def_side).tactics);
                let foul_events = self.maybe_foul(
                    minute,
                    def_side,
                    &attacker,
                    &defender,
                    Zone::Midfield,
                    rng,
                    foul_mod,
                );
                let fouled = foul_events.iter().any(|e| e.event_type == EventType::Foul);
                events.extend(foul_events);
                if fouled {
                    // Fouled team (att_side) retains possession for the free kick
                    self.possession = att_side;
                    self.ball_zone = Zone::Midfield;
                    return events;
                }
            } else {
                let evt =
                    MatchEvent::new(minute, EventType::Interception, def_side, Zone::Midfield)
                        .with_player(&defender.id);
                self.events.push(evt.clone());
                events.push(evt);
            }
            self.possession = def_side;
            self.ball_zone = Zone::Midfield;
        }
        events
    }

    fn resolve_attacking_third<R: Rng>(
        &mut self,
        minute: u8,
        att_side: Side,
        def_side: Side,
        rng: &mut R,
    ) -> Vec<MatchEvent> {
        let attacker = self.snap_player(att_side, Position::Forward, rng);
        let defender = self.snap_player(def_side, Position::Defender, rng);
        let success = self.attacking_duel_chance(att_side, def_side, &attacker, &defender);
        let zone = Zone::attacking_third(att_side);

        if rng.random_range(0.0..1.0f64) < success {
            self.resolve_attacking_breakthrough(minute, att_side, def_side, zone, &attacker.id, rng)
        } else {
            self.resolve_attacking_turnover(
                minute, att_side, def_side, zone, &attacker, &defender, rng,
            )
        }
    }

    fn attacking_duel_chance(
        &self,
        att_side: Side,
        def_side: Side,
        attacker: &crate::shared::PlayerSnap,
        defender: &crate::shared::PlayerSnap,
    ) -> f64 {
        let att_raw = (attacker.dribbling as f64
            + attacker.pace as f64
            + attacker.agility as f64
            + attacker.composure as f64)
            / 4.0;
        let def_raw = (defender.defending as f64
            + defender.tackling as f64
            + defender.positioning as f64
            + defender.aerial as f64)
            / 4.0;
        let att_rating = self.condition_adjusted_skill(attacker, att_raw)
            * trait_bonus(attacker, TraitContext::Dribbling);
        let def_rating = self.condition_adjusted_skill(defender, def_raw)
            * trait_bonus(defender, TraitContext::Tackling)
            * self.short_handed_cover(def_side);

        let att_mod = play_style_modifier(
            self.team_ref(att_side).play_style,
            PlayStylePhase::Attack,
            true,
        ) * role_modifier(Position::Forward, attacker.role, PlayStylePhase::Attack);
        let def_mod = play_style_modifier(
            self.team_ref(def_side).play_style,
            PlayStylePhase::Defense,
            true,
        ) * role_modifier(Position::Defender, defender.role, PlayStylePhase::Defense);
        let att_eff = att_rating * att_mod * crate::shared::home_mod(att_side, &self.config);
        let def_eff = def_rating
            * def_mod
            * crate::shared::home_mod(def_side, &self.config)
            * tactics_shape_modifier(&self.team_ref(def_side).tactics)
            * tactics_width_versus_shape(
                &self.team_ref(att_side).tactics,
                &self.team_ref(def_side).tactics,
            );
        att_eff / (att_eff + def_eff)
    }

    fn resolve_attacking_breakthrough<R: Rng>(
        &mut self,
        minute: u8,
        att_side: Side,
        def_side: Side,
        zone: Zone,
        attacker_id: &str,
        rng: &mut R,
    ) -> Vec<MatchEvent> {
        let mut events = Vec::new();
        let cross_prob = tactics_cross_probability(&self.team_ref(att_side).tactics);
        let evt =
            MatchEvent::new(minute, EventType::Dribble, att_side, zone).with_player(attacker_id);
        self.events.push(evt.clone());
        events.push(evt);
        if rng.random_range(0.0..1.0f64) >= cross_prob {
            self.ball_zone = Zone::attacking_box(att_side);
            return events;
        }
        let cross_evt =
            MatchEvent::new(minute, EventType::Cross, att_side, zone).with_player(attacker_id);
        self.events.push(cross_evt.clone());
        events.push(cross_evt);
        let header = self.snap_player(att_side, Position::Forward, rng);
        let def_header = self.snap_player(def_side, Position::Defender, rng);
        let aerial_att = header.aerial as f64
            * self.line_on_the_pitch(att_side, Position::Defender, PlayStylePhase::Attack);
        let aerial_def = def_header.aerial as f64;
        let aerial_win = aerial_att / (aerial_att + aerial_def);
        if rng.random_range(0.0..1.0f64) < aerial_win {
            self.ball_zone = Zone::attacking_box(att_side);
            events.extend(self.resolve_shot(minute, att_side, ShotOrigin::Cross, rng));
        } else {
            let clear_evt = MatchEvent::new(minute, EventType::Clearance, def_side, zone)
                .with_player(&def_header.id);
            self.events.push(clear_evt.clone());
            events.push(clear_evt);
            self.possession = def_side;
            self.ball_zone = Zone::defensive_third(att_side);
        }
        events
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "preserves resolve_attacking_third turnover signature without extra context struct"
    )]
    fn resolve_attacking_turnover<R: Rng>(
        &mut self,
        minute: u8,
        att_side: Side,
        def_side: Side,
        zone: Zone,
        attacker: &crate::shared::PlayerSnap,
        defender: &crate::shared::PlayerSnap,
        rng: &mut R,
    ) -> Vec<MatchEvent> {
        let mut events = Vec::new();
        let is_tackle = rng.random_range(0.0..1.0f64) < 0.5;
        let fouled = if is_tackle {
            let evt1 = MatchEvent::new(minute, EventType::DribbleTackled, att_side, zone)
                .with_player(&attacker.id)
                .with_secondary(&defender.id);
            let evt2 = MatchEvent::new(minute, EventType::Tackle, def_side, zone)
                .with_player(&defender.id);
            self.events.push(evt1.clone());
            self.events.push(evt2.clone());
            events.push(evt1);
            events.push(evt2);
            let foul_mod = tactics_foul_modifier(&self.team_ref(def_side).tactics);
            let foul_events =
                self.maybe_foul(minute, def_side, attacker, defender, zone, rng, foul_mod);
            let was_fouled = foul_events.iter().any(|e| e.event_type == EventType::Foul);
            events.extend(foul_events);
            was_fouled
        } else {
            let evt = MatchEvent::new(minute, EventType::Clearance, def_side, zone)
                .with_player(&defender.id);
            self.events.push(evt.clone());
            events.push(evt);
            false
        };
        if fouled {
            // Fouled team (att_side) retains possession for the free kick in the attacking third
            self.possession = att_side;
            self.ball_zone = zone;
            events.extend(self.take_free_kick(minute, att_side, rng));
            return events;
        }
        if rng.random_range(0.0..1.0f64) < 0.25 {
            let evt = MatchEvent::new(minute, EventType::Corner, att_side, zone);
            self.events.push(evt.clone());
            events.push(evt);
            let corner = self.corner_delivery(att_side);
            if rng.random_range(0.0..1.0f64) < CORNER_INTO_THE_BOX * corner.quality {
                self.ball_zone = Zone::attacking_box(att_side);
                self.pending_set_piece = Some(corner);
                return events;
            }
        }
        self.possession = def_side;
        self.ball_zone = Zone::defensive_third(att_side);
        events
    }

    fn resolve_shot<R: Rng>(
        &mut self,
        minute: u8,
        att_side: Side,
        origin: ShotOrigin,
        rng: &mut R,
    ) -> Vec<MatchEvent> {
        let def_side = att_side.opposite();
        let zone = Zone::attacking_box(att_side);
        let (penalty_stopped, mut events) =
            self.resolve_box_foul(minute, att_side, def_side, zone, rng);
        if penalty_stopped {
            return events;
        }

        let shooter = self.snap_player(att_side, Position::Forward, rng);
        let assister = self.snap_player(att_side, Position::Midfielder, rng);
        let goalkeeper = self.snap_player(def_side, Position::Goalkeeper, rng);

        let shoot_raw =
            (shooter.shooting as f64 + shooter.composure as f64 + shooter.decisions as f64) / 3.0;
        // A designated taker's delivery makes the chance; otherwise the midfielder who did.
        let creation = match origin.taker() {
            Some(_) => origin.delivery(),
            None => assist_creation_factor(assister.role),
        };
        let shoot_rating = self.condition_adjusted_skill(&shooter, shoot_raw)
            * trait_bonus(&shooter, TraitContext::Shooting)
            * creation;
        let gk_rating = self.keeper_rating(&goalkeeper);
        let creator_id = origin.taker().unwrap_or(&assister.id).to_string();

        let accuracy =
            (self.config.shot_accuracy_base + (shoot_rating - 50.0) / 200.0).clamp(0.15, 0.85);

        if rng.random_range(0.0..1.0f64) > accuracy {
            events.extend(self.record_off_target_shot(
                minute,
                att_side,
                def_side,
                zone,
                origin,
                &shooter.id,
                &creator_id,
                shoot_rating,
                rng,
            ));
            return events;
        }

        let def_line_mod = tactics_defensive_conversion_mod(&self.team_ref(def_side).tactics);
        let conversion = (self.config.goal_conversion_base * def_line_mod
            / self.short_handed_cover(def_side)
            + (shoot_rating - gk_rating) / 150.0)
            .clamp(0.10, 0.70);

        events.extend(self.record_on_target_shot(
            minute,
            att_side,
            def_side,
            zone,
            origin,
            &shooter.id,
            &creator_id,
            shoot_rating,
            gk_rating,
            conversion,
            rng,
        ));
        events
    }

    fn resolve_box_foul<R: Rng>(
        &mut self,
        minute: u8,
        att_side: Side,
        def_side: Side,
        zone: Zone,
        rng: &mut R,
    ) -> (bool, Vec<MatchEvent>) {
        let mut events = Vec::new();
        // Box foul rate fixed at 3.6% per shot — independent of foul_probability (which tunes outfield fouls)
        if rng.random_range(0.0..1.0f64) >= 0.036 {
            return (false, events);
        }
        let fouler = self.snap_player(def_side, Position::Defender, rng);
        let fouled = self.snap_player(att_side, Position::Forward, rng);
        let foul_evt = MatchEvent::new(minute, EventType::Foul, def_side, zone)
            .with_player(&fouler.id)
            .with_secondary(&fouled.id)
            .with_detail(EventDetail::Foul {
                severity: foul_severity(fouler.aggression),
            });
        self.events.push(foul_evt.clone());
        events.push(foul_evt);

        if rng.random_range(0.0..1.0f64) < self.config.penalty_probability {
            let pen_evt = MatchEvent::new(minute, EventType::PenaltyAwarded, att_side, zone);
            self.events.push(pen_evt.clone());
            events.push(pen_evt);
            events.extend(self.resolve_in_match_penalty(minute, att_side, rng));
            events.extend(self.maybe_card(minute, def_side, &fouler.id, zone, rng));
            self.ball_zone = Zone::Midfield;
            self.possession = def_side;
            return (true, events);
        }
        events.extend(self.maybe_card(minute, def_side, &fouler.id, zone, rng));
        (false, events)
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "shot helper avoids duplicating ShotRecord assembly"
    )]
    fn record_off_target_shot<R: Rng>(
        &mut self,
        minute: u8,
        att_side: Side,
        def_side: Side,
        zone: Zone,
        origin: ShotOrigin,
        shooter_id: &str,
        creator_id: &str,
        shoot_rating: f64,
        rng: &mut R,
    ) -> Vec<MatchEvent> {
        let mut events = Vec::new();
        let detail = EventDetail::Shot {
            danger: danger_band(shoot_rating),
        };
        let outcome = if rng.random_range(0.0..1.0f64) < 0.4 {
            let evt = MatchEvent::new(minute, EventType::ShotBlocked, att_side, zone)
                .with_player(shooter_id)
                .with_detail(detail);
            self.events.push(evt.clone());
            events.push(evt);
            self.ball_zone = Zone::Midfield;
            self.possession = def_side;
            ShotOutcome::Blocked
        } else {
            let evt = MatchEvent::new(minute, EventType::ShotOffTarget, att_side, zone)
                .with_player(shooter_id)
                .with_detail(detail);
            self.events.push(evt.clone());
            events.push(evt);
            let gk_evt = MatchEvent::new(minute, EventType::GoalKick, def_side, zone);
            self.events.push(gk_evt.clone());
            events.push(gk_evt);
            self.ball_zone = Zone::defensive_third(def_side);
            self.possession = def_side;
            ShotOutcome::OffTarget
        };
        self.record_shot(ShotRecord {
            minute,
            side: att_side,
            shooter_id,
            body_part: origin.body_part(),
            outcome,
            assisted_by: Some(creator_id.to_string()),
            sharpness: (shoot_rating / 100.0) as f32,
        });
        events
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "shot helper avoids duplicating ShotRecord assembly"
    )]
    fn record_on_target_shot<R: Rng>(
        &mut self,
        minute: u8,
        att_side: Side,
        def_side: Side,
        zone: Zone,
        origin: ShotOrigin,
        shooter_id: &str,
        creator_id: &str,
        shoot_rating: f64,
        gk_rating: f64,
        conversion: f64,
        rng: &mut R,
    ) -> Vec<MatchEvent> {
        let mut events = Vec::new();
        let outcome = if rng.random_range(0.0..1.0f64) < conversion {
            let context = self.goal_context(att_side);
            let evt = MatchEvent::new(minute, EventType::Goal, att_side, zone)
                .with_player(shooter_id)
                .with_secondary(creator_id)
                .with_detail(EventDetail::Goal { context });
            self.events.push(evt.clone());
            events.push(evt);
            self.add_goal(att_side);
            self.ball_zone = Zone::Midfield;
            self.possession = def_side;
            ShotOutcome::Goal
        } else {
            let evt = MatchEvent::new(minute, EventType::ShotSaved, att_side, zone)
                .with_player(shooter_id)
                .with_detail(EventDetail::Save {
                    quality: save_quality(gk_rating),
                });
            self.events.push(evt.clone());
            events.push(evt);
            if rng.random_range(0.0..1.0f64) < 0.40 {
                let corner_evt = MatchEvent::new(minute, EventType::Corner, att_side, zone);
                self.events.push(corner_evt.clone());
                events.push(corner_evt);
                self.possession = att_side;
                self.ball_zone = Zone::attacking_box(att_side);
                self.pending_set_piece = Some(self.corner_delivery(att_side));
            } else {
                let gk_evt = MatchEvent::new(minute, EventType::GoalKick, def_side, zone);
                self.events.push(gk_evt.clone());
                events.push(gk_evt);
                self.ball_zone = Zone::defensive_third(def_side);
                self.possession = def_side;
            }
            ShotOutcome::Saved
        };
        self.record_shot(ShotRecord {
            minute,
            side: att_side,
            shooter_id,
            body_part: origin.body_part(),
            outcome,
            assisted_by: Some(creator_id.to_string()),
            sharpness: (shoot_rating / 100.0) as f32,
        });
        events
    }

    /// What `side`'s players of `line` still on the pitch add in `phase` from their roles.
    fn line_on_the_pitch(&self, side: Side, line: Position, phase: PlayStylePhase) -> f64 {
        let on_the_pitch = self
            .team_ref(side)
            .players
            .iter()
            .filter(|player| !self.sent_off.contains(&player.id));
        line_role_factor(line, phase, on_the_pitch)
    }

    // -----------------------------------------------------------------------
    // Foul / card / penalty
    // -----------------------------------------------------------------------

    // Mirrors `engine::fouls::maybe_foul` — foul resolution needs the full context
    // (who, whom, where, when, tactics, RNG), so the argument count is inherent.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn maybe_foul<R: Rng>(
        &mut self,
        minute: u8,
        fouling_side: Side,
        fouled: &PlayerSnap,
        fouler: &PlayerSnap,
        zone: Zone,
        rng: &mut R,
        tactics_mod: f64,
    ) -> Vec<MatchEvent> {
        let mut events = Vec::new();

        let aggression_mod = fouler.aggression as f64 / 100.0;
        let foul_chance = self.config.foul_probability
            * (0.6 + aggression_mod * 0.8)
            * trait_bonus(fouler, TraitContext::Foul)
            * tactics_mod
            * self.foul_temper(fouling_side);
        if rng.random_range(0.0..1.0f64) >= foul_chance {
            return events;
        }

        let evt = MatchEvent::new(minute, EventType::Foul, fouling_side, zone)
            .with_player(&fouler.id)
            .with_secondary(&fouled.id)
            .with_detail(EventDetail::Foul {
                severity: foul_severity(fouler.aggression),
            });
        self.events.push(evt.clone());
        events.push(evt);

        let att_side = fouling_side.opposite();

        if zone.is_box_for(att_side)
            && rng.random_range(0.0..1.0f64) < self.config.penalty_probability
        {
            let evt = MatchEvent::new(minute, EventType::PenaltyAwarded, att_side, zone);
            self.events.push(evt.clone());
            events.push(evt);
            let pen_events = self.resolve_in_match_penalty(minute, att_side, rng);
            events.extend(pen_events);
        } else {
            let evt = MatchEvent::new(minute, EventType::FreeKick, att_side, zone);
            self.events.push(evt.clone());
            events.push(evt);
        }

        let card_events = self.maybe_card(minute, fouling_side, &fouler.id, zone, rng);
        events.extend(card_events);

        if rng.random_range(0.0..1.0f64) < self.config.injury_probability * fouled.injury_risk {
            let evt =
                MatchEvent::new(minute, EventType::Injury, att_side, zone).with_player(&fouled.id);
            self.events.push(evt.clone());
            events.push(evt);
        }

        events
    }

    pub(super) fn maybe_card<R: Rng>(
        &mut self,
        minute: u8,
        side: Side,
        fouler_id: &str,
        zone: Zone,
        rng: &mut R,
    ) -> Vec<MatchEvent> {
        let mut events = Vec::new();

        let aggression_factor = self
            .team_ref(side)
            .players
            .iter()
            .find(|p| p.id == fouler_id)
            .map(|p| p.aggression as f64 / 100.0)
            .unwrap_or(0.5);
        let booked = self.yellows.contains_key(fouler_id);
        let caution = if booked {
            crate::shared::BOOKED_CARD_FACTOR
        } else {
            1.0
        };
        let card_chance = self.config.yellow_card_probability
            * (0.5 + aggression_factor)
            * caution
            * self.card_temper(side);
        if rng.random_range(0.0..1.0f64) >= card_chance {
            return events;
        }

        if rng.random_range(0.0..1.0f64) < self.config.red_card_probability {
            let evt =
                MatchEvent::new(minute, EventType::RedCard, side, zone).with_player(fouler_id);
            self.events.push(evt.clone());
            events.push(evt);
            self.sent_off.insert(fouler_id.to_string());
            return events;
        }

        let current_yellows = self.yellows.entry(fouler_id.to_string()).or_insert(0);
        *current_yellows += 1;

        if *current_yellows >= 2 {
            let evt =
                MatchEvent::new(minute, EventType::SecondYellow, side, zone).with_player(fouler_id);
            self.events.push(evt.clone());
            events.push(evt);
            self.sent_off.insert(fouler_id.to_string());
        } else {
            let evt =
                MatchEvent::new(minute, EventType::YellowCard, side, zone).with_player(fouler_id);
            self.events.push(evt.clone());
            events.push(evt);
        }

        events
    }
}

#[cfg(test)]
mod event_detail_tests {
    use rand::SeedableRng;
    use rand::rngs::StdRng;

    use crate::event::{EventDetail, EventType, GoalContext};
    use crate::live_match::LiveMatchState;
    use crate::types::{MatchConfig, PlayStyle, PlayerData, Position, TeamData};

    fn make_player(id: &str, pos: Position) -> PlayerData {
        PlayerData {
            id: id.to_string(),
            name: id.to_string(),
            position: pos,
            ovr: 70,
            condition: 90,
            fitness: 75,
            injury_risk: 1.0,
            line_familiarity: [1.0; 4],
            morale_factor: 1.0,
            pace: 70,
            stamina: 70,
            strength: 70,
            agility: 70,
            passing: 70,
            shooting: 70,
            tackling: 70,
            dribbling: 70,
            defending: 70,
            positioning: 70,
            vision: 70,
            decisions: 70,
            composure: 70,
            aggression: 70,
            teamwork: 70,
            leadership: 70,
            handling: 70,
            reflexes: 70,
            aerial: 70,
            traits: vec![],
            role: crate::types::PlayerRole::Standard,
        }
    }

    fn make_team(id: &str) -> TeamData {
        TeamData {
            id: id.to_string(),
            name: id.to_string(),
            formation: "4-4-2".to_string(),
            play_style: PlayStyle::Balanced,
            tactics: crate::types::TacticsConfig::default(),
            players: vec![
                make_player(&format!("{id}_gk"), Position::Goalkeeper),
                make_player(&format!("{id}_d1"), Position::Defender),
                make_player(&format!("{id}_d2"), Position::Defender),
                make_player(&format!("{id}_d3"), Position::Defender),
                make_player(&format!("{id}_d4"), Position::Defender),
                make_player(&format!("{id}_m1"), Position::Midfielder),
                make_player(&format!("{id}_m2"), Position::Midfielder),
                make_player(&format!("{id}_m3"), Position::Midfielder),
                make_player(&format!("{id}_m4"), Position::Midfielder),
                make_player(&format!("{id}_f1"), Position::Forward),
                make_player(&format!("{id}_f2"), Position::Forward),
            ],
        }
    }

    /// The first goal of any match must be classified as `Opener` because both
    /// scores are 0 at that point.
    #[test]
    fn first_goal_detail_is_opener() {
        // Try multiple seeds and validate the invariant whenever a goal appears.
        let mut saw_any_goal = false;
        for seed in 0u64..500 {
            let mut state = LiveMatchState::new(
                make_team("home"),
                make_team("away"),
                MatchConfig::default(),
                vec![],
                vec![],
                false,
            );
            let mut rng = StdRng::seed_from_u64(seed);

            // Step minute-by-minute until finished or the first scoring event
            // appears. A `PenaltyGoal` can score before any open-play `Goal` and
            // updates the score, so the first open-play goal is only guaranteed
            // to be an `Opener` when nothing scored before it.
            let first_scoring = loop {
                let result = state.step_minute(&mut rng);
                let scoring = result
                    .events
                    .iter()
                    .find(|e| matches!(e.event_type, EventType::Goal | EventType::PenaltyGoal))
                    .cloned();
                if let Some(evt) = scoring {
                    break Some(evt);
                }
                if result.is_finished {
                    break None;
                }
            };

            if let Some(first_evt) = first_scoring
                && first_evt.event_type == EventType::Goal
            {
                assert_eq!(
                    first_evt.detail,
                    Some(EventDetail::Goal {
                        context: GoalContext::Opener
                    }),
                    "seed {seed}: first goal detail should be Opener, got {:?}",
                    first_evt.detail
                );
                saw_any_goal = true;
            }
            // No goal scored in this seed — try the next one.
        }
        assert!(
            saw_any_goal,
            "No goal was scored in 500 seeds; increase seed range or check engine config"
        );
    }
}
