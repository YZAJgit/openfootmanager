import { invoke } from "@tauri-apps/api/core";

import type { PlayerCharacterView } from "../store/generated/PlayerCharacterView";
import type { TeamChemistryView } from "../store/generated/TeamChemistryView";

/**
 * One player's character labels and traits as the manager's club is allowed to see them.
 * Unknown players return `{ known: false, labels: [], profile: null }`; missing ids return `null`.
 */
export async function getPlayerCharacter(playerId: string): Promise<PlayerCharacterView | null> {
  return invoke<PlayerCharacterView | null>("get_player_character", { playerId });
}

/**
 * On-pitch cohesion, tactical familiarity, key partnerships, and assistant notes for the active XI.
 * Returns `null` when the manager has no club.
 */
export async function getTeamChemistry(): Promise<TeamChemistryView | null> {
  return invoke<TeamChemistryView | null>("get_team_chemistry");
}
