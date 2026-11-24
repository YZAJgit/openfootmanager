import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";

import {
  clearIndividualDevelopmentPlan,
  getIndividualPlanOptions,
  getTrainingFocusAttributes,
  setIndividualDevelopmentPlan,
  setPlayerTrainingFocus,
  setTraining,
  setTrainingGroups,
  setTrainingSchedule,
} from "./trainingService";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

const mockedInvoke = vi.mocked(invoke);

describe("trainingService", () => {
  beforeEach(() => {
    mockedInvoke.mockReset();
  });

  it("calls the set training backend command", async () => {
    const response = { manager: { id: "manager-1" } };
    mockedInvoke.mockResolvedValueOnce(response);

    await expect(setTraining("Physical", "High")).resolves.toBe(response);
    expect(mockedInvoke).toHaveBeenCalledWith("set_training", {
      careerToken: 0,
      focus: "Physical",
      intensity: "High",
    });
  });

  it("calls the set training schedule backend command", async () => {
    const response = { manager: { id: "manager-1" } };
    mockedInvoke.mockResolvedValueOnce(response);

    await expect(setTrainingSchedule("Light")).resolves.toBe(response);
    expect(mockedInvoke).toHaveBeenCalledWith("set_training_schedule", {
      careerToken: 0,
      schedule: "Light",
    });
  });

  it("calls the set training groups backend command", async () => {
    const response = { manager: { id: "manager-1" } };
    const groups = [{ id: "grp-1", name: "Attack", focus: "Attacking", player_ids: ["player-1"] }];
    mockedInvoke.mockResolvedValueOnce(response);

    await expect(setTrainingGroups(groups)).resolves.toBe(response);
    expect(mockedInvoke).toHaveBeenCalledWith("set_training_groups", { careerToken: 0, groups });
  });

  it("calls the set player training focus backend command", async () => {
    const response = { manager: { id: "manager-1" } };
    mockedInvoke.mockResolvedValueOnce(response);

    await expect(setPlayerTrainingFocus("player-1", null)).resolves.toBe(response);
    expect(mockedInvoke).toHaveBeenCalledWith("set_player_training_focus", {
      careerToken: 0,
      playerId: "player-1",
      focus: null,
    });
  });

  it("loads the focus attribute table from the backend", async () => {
    const table = [{ focus: "Technical", outfield: ["passing"], goalkeeper: ["handling"] }];
    mockedInvoke.mockResolvedValueOnce(table);

    await expect(getTrainingFocusAttributes()).resolves.toEqual(table);
    expect(mockedInvoke).toHaveBeenCalledWith("get_training_focus_attributes");
  });

  it("loads individual development plan options from the backend", async () => {
    const options = {
      max_plans: 3,
      eligible_mentors: [{ id: "m1", match_name: "M. Veteran", age: 33 }],
      eligible_mentees: [{ id: "y1", match_name: "Y. Prospect", age: 19 }],
    };
    mockedInvoke.mockResolvedValueOnce(options);

    await expect(getIndividualPlanOptions()).resolves.toEqual(options);
    expect(mockedInvoke).toHaveBeenCalledWith("get_individual_plan_options");
  });

  it("calls the set individual development plan backend command with career token", async () => {
    const response = { manager: { id: "manager-1" } };
    mockedInvoke.mockResolvedValueOnce(response);

    await expect(
      setIndividualDevelopmentPlan("player-1", "Technical", "High", "mentor-1"),
    ).resolves.toBe(response);
    expect(mockedInvoke).toHaveBeenCalledWith("set_individual_development_plan", {
      careerToken: 0,
      playerId: "player-1",
      focus: "Technical",
      intensity: "High",
      mentorId: "mentor-1",
    });
  });

  it("calls the clear individual development plan backend command with career token", async () => {
    const response = { manager: { id: "manager-1" } };
    mockedInvoke.mockResolvedValueOnce(response);

    await expect(clearIndividualDevelopmentPlan("player-1")).resolves.toBe(response);
    expect(mockedInvoke).toHaveBeenCalledWith("clear_individual_development_plan", {
      careerToken: 0,
      playerId: "player-1",
    });
  });
});
