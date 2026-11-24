import { invoke } from "@tauri-apps/api/core";
import type { GameStateData } from "../store/gameStore";
import type { YouthAcademyOverview } from "../store/generated/YouthAcademyOverview";
import { invokeForCareer } from "./careerInvoke";

export function getYouthAcademyOverview(): Promise<YouthAcademyOverview | null> {
  return invoke("get_youth_academy_overview");
}

export function decideScholarshipAction(
  playerId: string,
  offerPro: boolean,
): Promise<GameStateData> {
  return invokeForCareer("decide_scholarship_action", { playerId, offerPro });
}

export function recallYouthLoanBreach(playerId: string): Promise<GameStateData> {
  return invokeForCareer("recall_youth_loan_breach", { playerId });
}
