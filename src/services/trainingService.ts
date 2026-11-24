import { invoke } from "@tauri-apps/api/core";

import type { GameStateData } from "../store/gameStore";
import type { IndividualPlanOptions } from "../store/generated/IndividualPlanOptions";
import type { TrainingFocusAttributesData, TrainingGroupData } from "../store/types";
import { invokeForCareer } from "./careerInvoke";

// Lives in `store/types.ts` with the rest of the wire shape — `TeamData.training_groups` needs
// it, and a type the store depends on cannot sit in a service the store does not import.
// Re-exported here so existing imports keep working.
export type { TrainingGroupData } from "../store/types";
export type { IndividualPlanOptions } from "../store/generated/IndividualPlanOptions";

export async function setTraining(focus: string, intensity: string): Promise<GameStateData> {
  return invokeForCareer<GameStateData>("set_training", {
    focus,
    intensity,
  });
}

export async function setTrainingSchedule(schedule: string): Promise<GameStateData> {
  return invokeForCareer<GameStateData>("set_training_schedule", {
    schedule,
  });
}

export async function setTrainingGroups(groups: TrainingGroupData[]): Promise<GameStateData> {
  return invokeForCareer<GameStateData>("set_training_groups", {
    groups,
  });
}

export async function setPlayerTrainingFocus(
  playerId: string,
  focus: string | null,
): Promise<GameStateData> {
  return invokeForCareer<GameStateData>("set_player_training_focus", {
    playerId,
    focus,
  });
}

export async function getTrainingFocusAttributes(): Promise<TrainingFocusAttributesData[]> {
  return invoke<TrainingFocusAttributesData[]>("get_training_focus_attributes");
}

export async function getIndividualPlanOptions(): Promise<IndividualPlanOptions> {
  return invoke<IndividualPlanOptions>("get_individual_plan_options");
}

export async function setIndividualDevelopmentPlan(
  playerId: string,
  focus: string,
  intensity: string,
  mentorId: string | null,
): Promise<GameStateData> {
  return invokeForCareer<GameStateData>("set_individual_development_plan", {
    playerId,
    focus,
    intensity,
    mentorId,
  });
}

export async function clearIndividualDevelopmentPlan(playerId: string): Promise<GameStateData> {
  return invokeForCareer<GameStateData>("clear_individual_development_plan", {
    playerId,
  });
}
