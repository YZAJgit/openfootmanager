import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";

import { i18nReady } from "../../i18n";
import type { GameStateData, PlayerData, TeamData } from "../../store/gameStore";
import { createSeasonContext } from "../../test-utils/factories";
import IndividualPlansCard from "./IndividualPlansCard";

const getIndividualPlanOptionsMock = vi.fn();
const setIndividualDevelopmentPlanMock = vi.fn();
const clearIndividualDevelopmentPlanMock = vi.fn();

vi.mock("../../services/trainingService", () => ({
  getIndividualPlanOptions: (...args: unknown[]) => getIndividualPlanOptionsMock(...args),
  setIndividualDevelopmentPlan: (...args: unknown[]) => setIndividualDevelopmentPlanMock(...args),
  clearIndividualDevelopmentPlan: (...args: unknown[]) =>
    clearIndividualDevelopmentPlanMock(...args),
}));

vi.mock("react-i18next", () => ({
  initReactI18next: { type: "3rdParty", init: () => undefined },
  useTranslation: () => ({
    t: (key: string, params?: Record<string, string | number>, fallback?: string) => {
      if (key === "training.individualPlans.title") return "Individual Development Plans";
      if (key === "training.individualPlans.description") return `Up to ${params?.max} plans`;
      if (key === "training.individualPlans.slotCount")
        return `${params?.count} / ${params?.max} plans`;
      if (key === "training.individualPlans.empty") return "No individual plans active";
      if (key === "training.individualPlans.selectPlayer") return "Select a player";
      if (key === "training.individualPlans.mentorLabel") return "Senior mentor (optional)";
      if (key === "training.individualPlans.noMentor") return "No mentor";
      if (key === "training.individualPlans.mentorOption")
        return `${params?.name} (Age ${params?.age})`;
      if (key === "training.individualPlans.mentorTag") return `Mentored by ${params?.mentor}`;
      if (key === "training.individualPlans.menteeAgeHint") return "Mentoring for 21 or under";
      if (key === "training.individualPlans.noEligibleMentors") return "No senior mentors";
      if (key === "training.individualPlans.addPlan") return "Add Plan";
      if (key === "training.individualPlans.updatePlan") return "Update Plan";
      if (key === "training.individualPlans.editPlan") return "Edit";
      if (key === "training.individualPlans.clearPlan") return "Remove";
      if (key === "training.individualPlans.limitReached")
        return `All ${params?.max} plan slots used`;
      if (key === "common.player") return "Player";
      if (key === "training.effectiveFocus") return "Focus";
      if (key === "training.intensity") return "Intensity";
      if (key.startsWith("training.focuses.")) return key.replace("training.focuses.", "");
      if (key.startsWith("training.intensities.")) return key.replace("training.intensities.", "");
      return fallback ?? key;
    },
  }),
}));

beforeAll(async () => {
  await i18nReady;
});

function createTeam(overrides: Partial<TeamData> = {}): TeamData {
  return {
    id: "team-1",
    name: "Alpha FC",
    short_name: "ALP",
    country: "GB",
    city: "London",
    stadium_name: "Alpha Ground",
    stadium_capacity: 30000,
    finance: 500000,
    manager_id: "manager-1",
    reputation: 50,
    wage_budget: 50000,
    transfer_budget: 250000,
    season_income: 0,
    season_expenses: 0,
    formation: "4-4-2",
    play_style: "Balanced",
    training_focus: "Physical",
    training_intensity: "Medium",
    training_schedule: "Balanced",
    founded_year: 1900,
    colors: { primary: "#000000", secondary: "#ffffff" },
    starting_xi_ids: [],
    form: [],
    history: [],
    ...overrides,
  };
}

function createPlayer(overrides: Partial<PlayerData> = {}): PlayerData {
  return {
    id: "young-1",
    match_name: "Y. Prospect",
    full_name: "Young Prospect",
    date_of_birth: "2007-01-01",
    nationality: "GB",
    position: "Forward",
    natural_position: "Forward",
    alternate_positions: [],
    training_focus: null,
    attributes: {
      pace: 60,
      stamina: 60,
      strength: 60,
      agility: 60,
      passing: 60,
      shooting: 60,
      tackling: 60,
      dribbling: 60,
      defending: 60,
      positioning: 60,
      vision: 60,
      decisions: 60,
      composure: 60,
      aggression: 60,
      teamwork: 60,
      leadership: 60,
      handling: 20,
      reflexes: 20,
      aerial: 60,
    },
    condition: 80,
    morale: 75,
    injury: null,
    team_id: "team-1",
    retired: false,
    contract_end: "2028-06-30",
    wage: 12000,
    market_value: 350000,
    stats: {
      appearances: 0,
      goals: 0,
      assists: 0,
      clean_sheets: 0,
      yellow_cards: 0,
      red_cards: 0,
      avg_rating: 0,
      minutes_played: 0,
    },
    career: [],
    transfer_listed: false,
    loan_listed: false,
    transfer_offers: [],
    traits: [],
    ...overrides,
  };
}

function createGameState(team: TeamData): GameStateData {
  return {
    season_context: createSeasonContext(),
    clock: {
      current_date: "2026-08-11T00:00:00Z",
      start_date: "2026-07-01T00:00:00Z",
    },
    manager: {
      display_name: "Jane Doe",
      id: "manager-1",
      first_name: "Jane",
      last_name: "Doe",
      date_of_birth: "1980-01-01",
      nationality: "GB",
      reputation: 50,
      satisfaction: 50,
      fan_approval: 50,
      team_id: "team-1",
      career_stats: {
        matches_managed: 0,
        wins: 0,
        draws: 0,
        losses: 0,
        trophies: 0,
        best_finish: null,
      },
      career_history: [],
    },
    teams: [team],
    players: [],
    staff: [],
    messages: [],
    news: [],
    league: null,
    scouting_assignments: [],
    board_objectives: [],
  };
}

const DEFAULT_OPTIONS = {
  max_plans: 3,
  eligible_mentors: [{ id: "mentor-1", match_name: "M. Veteran", age: 33 }],
  eligible_mentees: [{ id: "young-1", match_name: "Y. Prospect", age: 19 }],
};

describe("IndividualPlansCard", () => {
  beforeEach(() => {
    getIndividualPlanOptionsMock.mockReset();
    setIndividualDevelopmentPlanMock.mockReset();
    clearIndividualDevelopmentPlanMock.mockReset();
    getIndividualPlanOptionsMock.mockResolvedValue(DEFAULT_OPTIONS);
  });

  /**
   * Given a club with an active mentored plan and an ordinary squad group
   * When the card renders
   * Then it displays the active plan count, focus, intensity, mentor name and ignores squad groups.
   */
  it("renders active individual plans with count and mentor and ignores squad groups", async () => {
    const team = createTeam({
      training_groups: [
        { id: "grp-1", name: "Defenders", focus: "Defending", player_ids: ["mentor-1"] },
        {
          id: "individual:young-1",
          name: "Y. Prospect",
          focus: "Technical",
          player_ids: ["young-1"],
          intensity: "High",
          mentor_id: "mentor-1",
        },
      ],
    });
    const roster = [
      createPlayer({ id: "young-1", match_name: "Y. Prospect" }),
      createPlayer({ id: "mentor-1", match_name: "M. Veteran", date_of_birth: "1993-01-01" }),
    ];

    render(
      <IndividualPlansCard
        team={team}
        roster={roster}
        isSaving={false}
        setIsSaving={vi.fn()}
        trainingFocusIds={["Physical", "Technical"]}
        intensityIds={["Low", "Medium", "High"]}
      />,
    );

    expect(await screen.findByText("1 / 3 plans")).toBeInTheDocument();
    expect(screen.getByText("Y. Prospect")).toBeInTheDocument();
    expect(screen.getByText("Technical.label")).toBeInTheDocument();
    expect(screen.getByText("High.label")).toBeInTheDocument();
    expect(screen.getByText("Mentored by M. Veteran")).toBeInTheDocument();
    expect(screen.queryByText("Defenders")).not.toBeInTheDocument();
  });

  /**
   * Given an eligible mentee and an eligible senior mentor from backend options
   * When the manager picks the player, focus, intensity and mentor and clicks Add Plan
   * Then `setIndividualDevelopmentPlan` is called with those arguments and the updated state is forwarded.
   */
  it("creates an individual plan with a mentor for an eligible mentee", async () => {
    const updatedState = createGameState(createTeam());
    const onGameUpdate = vi.fn();
    setIndividualDevelopmentPlanMock.mockResolvedValueOnce(updatedState);

    const roster = [
      createPlayer({ id: "young-1", match_name: "Y. Prospect" }),
      createPlayer({ id: "mentor-1", match_name: "M. Veteran", date_of_birth: "1993-01-01" }),
    ];

    render(
      <IndividualPlansCard
        team={createTeam()}
        onGameUpdate={onGameUpdate}
        roster={roster}
        isSaving={false}
        setIsSaving={vi.fn()}
        trainingFocusIds={["Physical", "Technical"]}
        intensityIds={["Low", "Medium", "High"]}
      />,
    );

    await screen.findByText("0 / 3 plans");

    fireEvent.click(screen.getByRole("combobox", { name: "Player" }));
    fireEvent.click(screen.getByRole("option", { name: "Y. Prospect" }));

    fireEvent.click(screen.getByRole("combobox", { name: "Focus" }));
    fireEvent.click(screen.getByRole("option", { name: "Technical.label" }));

    fireEvent.click(screen.getByRole("combobox", { name: "Intensity" }));
    fireEvent.click(screen.getByRole("option", { name: "High.label" }));

    fireEvent.click(screen.getByRole("combobox", { name: "Senior mentor (optional)" }));
    fireEvent.click(screen.getByRole("option", { name: "M. Veteran (Age 33)" }));

    fireEvent.click(screen.getByRole("button", { name: "Add Plan" }));

    await waitFor(() => {
      expect(setIndividualDevelopmentPlanMock).toHaveBeenCalledWith(
        "young-1",
        "Technical",
        "High",
        "mentor-1",
      );
      expect(onGameUpdate).toHaveBeenCalledWith(updatedState);
    });
  });

  /**
   * Given a club already at 3 / 3 plans
   * When an unplanned player is selected vs an already planned player
   * Then adding a 4th plan is disabled, while editing an existing plan remains enabled.
   */
  it("disables adding a fourth plan at the cap while allowing updates to existing plans", async () => {
    const team = createTeam({
      training_groups: [
        {
          id: "individual:p1",
          name: "A. One",
          focus: "Physical",
          player_ids: ["p1"],
          intensity: "Medium",
        },
        {
          id: "individual:p2",
          name: "B. Two",
          focus: "Technical",
          player_ids: ["p2"],
          intensity: "High",
        },
        {
          id: "individual:p3",
          name: "C. Three",
          focus: "Tactical",
          player_ids: ["p3"],
          intensity: "Low",
        },
      ],
    });
    const roster = [
      createPlayer({ id: "p1", match_name: "A. One" }),
      createPlayer({ id: "p2", match_name: "B. Two" }),
      createPlayer({ id: "p3", match_name: "C. Three" }),
      createPlayer({ id: "p4", match_name: "D. Four" }),
    ];

    render(
      <IndividualPlansCard
        team={team}
        roster={roster}
        isSaving={false}
        setIsSaving={vi.fn()}
        trainingFocusIds={["Physical", "Technical", "Tactical"]}
        intensityIds={["Low", "Medium", "High"]}
      />,
    );

    expect(await screen.findByText("3 / 3 plans")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("combobox", { name: "Player" }));
    fireEvent.click(screen.getByRole("option", { name: "D. Four" }));
    expect(screen.getByRole("button", { name: "Add Plan" })).toBeDisabled();
    expect(screen.getByText("All 3 plan slots used")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Edit A. One" }));
    expect(screen.getByRole("button", { name: "Update Plan" })).toBeEnabled();
  });

  /**
   * Given an active plan
   * When the manager clicks Remove
   * Then `clearIndividualDevelopmentPlan` is called and the updated state is forwarded.
   */
  it("clears an active individual plan when Remove is clicked", async () => {
    const updatedState = createGameState(createTeam());
    const onGameUpdate = vi.fn();
    clearIndividualDevelopmentPlanMock.mockResolvedValueOnce(updatedState);

    const team = createTeam({
      training_groups: [
        {
          id: "individual:young-1",
          name: "Y. Prospect",
          focus: "Technical",
          player_ids: ["young-1"],
          intensity: "High",
        },
      ],
    });

    render(
      <IndividualPlansCard
        team={team}
        onGameUpdate={onGameUpdate}
        roster={[createPlayer({ id: "young-1", match_name: "Y. Prospect" })]}
        isSaving={false}
        setIsSaving={vi.fn()}
        trainingFocusIds={["Physical", "Technical"]}
        intensityIds={["Low", "Medium", "High"]}
      />,
    );

    fireEvent.click(await screen.findByRole("button", { name: "Remove Y. Prospect" }));

    await waitFor(() => {
      expect(clearIndividualDevelopmentPlanMock).toHaveBeenCalledWith("young-1");
      expect(onGameUpdate).toHaveBeenCalledWith(updatedState);
    });
  });

  /**
   * Given a backend error key returned when saving a plan
   * When the save fails
   * Then the translated message is rendered in an alert banner.
   */
  it("resolves and displays backend error keys cleanly", async () => {
    setIndividualDevelopmentPlanMock.mockRejectedValueOnce(
      "be.error.training.individual_plan_limit",
    );

    render(
      <IndividualPlansCard
        team={createTeam()}
        roster={[createPlayer({ id: "young-1", match_name: "Y. Prospect" })]}
        isSaving={false}
        setIsSaving={vi.fn()}
        trainingFocusIds={["Physical", "Technical"]}
        intensityIds={["Low", "Medium", "High"]}
      />,
    );

    await screen.findByText("0 / 3 plans");
    fireEvent.click(screen.getByRole("combobox", { name: "Player" }));
    fireEvent.click(screen.getByRole("option", { name: "Y. Prospect" }));
    fireEvent.click(screen.getByRole("button", { name: "Add Plan" }));

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent(
      "A club can run at most 3 individual development plans at once.",
    );
  });

  /**
   * Given an active mentored plan while plan options are still loading
   * When the manager opens the plan for editing before options resolve
   * Then the Save button stays disabled until options finish loading and preserves the mentor on save.
   */
  it("disables saving while individual plan options are pending and preserves mentor once loaded", async () => {
    let resolveOptions!: (value: typeof DEFAULT_OPTIONS) => void;
    const pendingOptions = new Promise<typeof DEFAULT_OPTIONS>((resolve) => {
      resolveOptions = resolve;
    });
    getIndividualPlanOptionsMock.mockReturnValueOnce(pendingOptions);
    const updatedState = createGameState(createTeam());
    setIndividualDevelopmentPlanMock.mockResolvedValueOnce(updatedState);

    const team = createTeam({
      training_groups: [
        {
          id: "individual:young-1",
          name: "Y. Prospect",
          focus: "Technical",
          player_ids: ["young-1"],
          intensity: "High",
          mentor_id: "mentor-1",
        },
      ],
    });
    const roster = [
      createPlayer({ id: "young-1", match_name: "Y. Prospect" }),
      createPlayer({ id: "mentor-1", match_name: "M. Veteran", date_of_birth: "1993-01-01" }),
    ];

    render(
      <IndividualPlansCard
        team={team}
        roster={roster}
        isSaving={false}
        setIsSaving={vi.fn()}
        trainingFocusIds={["Physical", "Technical"]}
        intensityIds={["Low", "Medium", "High"]}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "Edit Y. Prospect" }));
    expect(screen.getByRole("button", { name: "Update Plan" })).toBeDisabled();

    resolveOptions(DEFAULT_OPTIONS);

    const updateButton = screen.getByRole("button", { name: "Update Plan" });
    await waitFor(() => {
      expect(updateButton).toBeEnabled();
    });

    fireEvent.click(updateButton);

    await waitFor(() => {
      expect(setIndividualDevelopmentPlanMock).toHaveBeenCalledWith(
        "young-1",
        "Technical",
        "High",
        "mentor-1",
      );
    });
  });

  /**
   * Given `getIndividualPlanOptions` fails to load
   * When the manager selects or edits a mentored plan
   * Then an error alert is displayed and the Save button remains disabled.
   */
  it("surfaces an error and disables saving when individual plan options fail to load", async () => {
    const consoleErrorSpy = vi.spyOn(console, "error").mockImplementation(() => undefined);
    getIndividualPlanOptionsMock.mockRejectedValueOnce("be.error.noActiveGameSession");

    const team = createTeam({
      training_groups: [
        {
          id: "individual:young-1",
          name: "Y. Prospect",
          focus: "Technical",
          player_ids: ["young-1"],
          intensity: "High",
          mentor_id: "mentor-1",
        },
      ],
    });
    const roster = [
      createPlayer({ id: "young-1", match_name: "Y. Prospect" }),
      createPlayer({ id: "mentor-1", match_name: "M. Veteran", date_of_birth: "1993-01-01" }),
    ];

    try {
      render(
        <IndividualPlansCard
          team={team}
          roster={roster}
          isSaving={false}
          setIsSaving={vi.fn()}
          trainingFocusIds={["Physical", "Technical"]}
          intensityIds={["Low", "Medium", "High"]}
        />,
      );

      const alert = await screen.findByRole("alert");
      expect(alert).toHaveTextContent("No active game session");

      fireEvent.click(screen.getByRole("button", { name: "Edit Y. Prospect" }));
      expect(screen.getByRole("button", { name: "Update Plan" })).toBeDisabled();
      expect(setIndividualDevelopmentPlanMock).not.toHaveBeenCalled();
    } finally {
      consoleErrorSpy.mockRestore();
    }
  });

  /**
   * Given an existing mentored plan and another eligible young player with no plan
   * When the manager edits the mentored plan and then switches to the unplanned young player
   * Then the mentor selector resets to empty and saving submits `mentor_id: null`.
   */
  it("resets mentor selection when switching from a mentored plan to an unplanned eligible mentee", async () => {
    getIndividualPlanOptionsMock.mockResolvedValueOnce({
      max_plans: 3,
      eligible_mentors: [{ id: "mentor-1", match_name: "M. Veteran", age: 33 }],
      eligible_mentees: [
        { id: "young-1", match_name: "Y. Prospect", age: 19 },
        { id: "young-2", match_name: "Z. Rookie", age: 18 },
      ],
    });
    const updatedState = createGameState(createTeam());
    setIndividualDevelopmentPlanMock.mockResolvedValueOnce(updatedState);

    const team = createTeam({
      training_groups: [
        {
          id: "individual:young-1",
          name: "Y. Prospect",
          focus: "Technical",
          player_ids: ["young-1"],
          intensity: "High",
          mentor_id: "mentor-1",
        },
      ],
    });
    const roster = [
      createPlayer({ id: "young-1", match_name: "Y. Prospect" }),
      createPlayer({ id: "young-2", match_name: "Z. Rookie", date_of_birth: "2008-01-01" }),
      createPlayer({ id: "mentor-1", match_name: "M. Veteran", date_of_birth: "1993-01-01" }),
    ];

    render(
      <IndividualPlansCard
        team={team}
        roster={roster}
        isSaving={false}
        setIsSaving={vi.fn()}
        trainingFocusIds={["Physical", "Technical"]}
        intensityIds={["Low", "Medium", "High"]}
      />,
    );

    await screen.findByText("1 / 3 plans");

    fireEvent.click(screen.getByRole("button", { name: "Edit Y. Prospect" }));
    expect(screen.getByRole("combobox", { name: "Senior mentor (optional)" })).toHaveTextContent(
      "M. Veteran (Age 33)",
    );

    fireEvent.click(screen.getByRole("combobox", { name: "Player" }));
    fireEvent.click(screen.getByRole("option", { name: "Z. Rookie" }));

    expect(screen.getByRole("combobox", { name: "Senior mentor (optional)" })).toHaveTextContent(
      "No mentor",
    );

    fireEvent.click(screen.getByRole("button", { name: "Add Plan" }));

    await waitFor(() => {
      expect(setIndividualDevelopmentPlanMock).toHaveBeenCalledWith(
        "young-2",
        "Technical",
        "High",
        null,
      );
    });
  });
});
