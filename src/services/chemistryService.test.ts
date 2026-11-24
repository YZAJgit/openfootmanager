import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { PlayerCharacterView } from "../store/generated/PlayerCharacterView";
import type { TeamChemistryView } from "../store/generated/TeamChemistryView";
import { getPlayerCharacter, getTeamChemistry } from "./chemistryService";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
const mockedInvoke = vi.mocked(invoke);

beforeEach(() => {
  mockedInvoke.mockReset();
});

describe("chemistryService", () => {
  /**
   * Given a known player in the active career
   * When his character is requested
   * Then the service invokes get_player_character with playerId and returns the projection as-is
   */
  it("fetches a player's character projection", async () => {
    const view: PlayerCharacterView = {
      known: true,
      labels: ["ModelProfessional", "Consistent"],
      profile: {
        professionalism: 88,
        ambition: 70,
        loyalty: 75,
        consistency: 84,
        big_matches: 72,
        pressure_handling: 78,
        injury_proneness: 30,
        adaptability: 66,
      },
    };
    mockedInvoke.mockResolvedValueOnce(view);

    await expect(getPlayerCharacter("p1")).resolves.toEqual(view);
    expect(mockedInvoke).toHaveBeenCalledWith("get_player_character", { playerId: "p1" });
  });

  /**
   * Given an unknown player id not present in the game
   * When his character is requested
   * Then null is returned unchanged
   */
  it("passes on null for a missing player", async () => {
    mockedInvoke.mockResolvedValueOnce(null);

    await expect(getPlayerCharacter("missing")).resolves.toBeNull();
  });

  /**
   * Given a managed club
   * When team chemistry is requested
   * Then the service invokes get_team_chemistry and returns the projection
   */
  it("fetches the active club's team chemistry", async () => {
    const view: TeamChemistryView = {
      cohesion_factor: 1.02,
      familiarity_factor: 1.01,
      formation: "4-4-2",
      tactical_familiarity: {
        formation: "4-4-2",
        familiarity: 75,
        matches_together: 12,
      },
      advice: [],
      top_pairs: [],
    };
    mockedInvoke.mockResolvedValueOnce(view);

    await expect(getTeamChemistry()).resolves.toEqual(view);
    expect(mockedInvoke).toHaveBeenCalledWith("get_team_chemistry");
  });

  /**
   * Given no active game session
   * When team chemistry is requested
   * Then the backend translation key error propagates to the caller
   */
  it("propagates backend errors", async () => {
    mockedInvoke.mockRejectedValueOnce("be.error.noActiveGameSession");

    await expect(getTeamChemistry()).rejects.toBe("be.error.noActiveGameSession");
  });
});
