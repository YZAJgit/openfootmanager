import { render, screen, waitFor, within } from "@testing-library/react";
import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";

import { i18nReady } from "../../i18n";
import { getTeamChemistry } from "../../services/chemistryService";
import type { TeamChemistryView } from "../../store/generated/TeamChemistryView";
import SquadChemistryCard from "./SquadChemistryCard";

vi.mock("../../services/chemistryService", () => ({
  getTeamChemistry: vi.fn(),
}));

const mockedGetTeamChemistry = vi.mocked(getTeamChemistry);

beforeAll(async () => {
  await i18nReady;
});

beforeEach(() => {
  mockedGetTeamChemistry.mockReset();
});

describe("SquadChemistryCard", () => {
  it("shows carried familiarity without the old formation match count", async () => {
    mockedGetTeamChemistry.mockResolvedValueOnce({
      cohesion_factor: 1,
      familiarity_factor: 1,
      formation: "4-3-3",
      tactical_familiarity: { formation: "4-4-2", familiarity: 75, matches_together: 7 },
      advice: [],
      top_pairs: [],
    });
    render(<SquadChemistryCard refreshKey="switch" />);
    const bar = await screen.findByRole("progressbar", { name: "Tactical Familiarity" });
    expect(bar).toHaveAttribute("aria-valuenow", "30");
    expect(screen.queryByText("7 matches together")).toBeNull();
  });
  it("ignores a stale response after the newer response", async () => {
    let resolveFirst: (value: TeamChemistryView | null) => void = () => {};
    mockedGetTeamChemistry.mockReturnValueOnce(
      new Promise((resolve) => {
        resolveFirst = resolve;
      }),
    );
    const view: TeamChemistryView = {
      cohesion_factor: 1,
      familiarity_factor: 1,
      formation: "4-3-3",
      tactical_familiarity: { formation: "", familiarity: 0, matches_together: 0 },
      advice: [],
      top_pairs: [],
    };
    mockedGetTeamChemistry.mockResolvedValueOnce(view);
    const { rerender } = render(<SquadChemistryCard refreshKey="first" />);
    rerender(<SquadChemistryCard refreshKey="second" />);
    await screen.findByText("4-3-3");
    resolveFirst({ ...view, formation: "4-4-2" });
    await waitFor(() => expect(screen.getByText("4-3-3")).toBeInTheDocument());
    expect(screen.queryByText("4-4-2")).toBeNull();
  });
  it("hides the previous team while a refresh is pending", async () => {
    const previous: TeamChemistryView = {
      cohesion_factor: 1,
      familiarity_factor: 1,
      formation: "4-4-2",
      tactical_familiarity: { formation: "", familiarity: 0, matches_together: 0 },
      advice: [],
      top_pairs: [],
    };
    mockedGetTeamChemistry.mockResolvedValueOnce(previous);
    mockedGetTeamChemistry.mockReturnValueOnce(new Promise(() => {}));
    const { container, rerender } = render(<SquadChemistryCard refreshKey="first" />);
    await screen.findByText("4-4-2");
    rerender(<SquadChemistryCard refreshKey="second" />);
    expect(container).toBeEmptyDOMElement();
  });
  /**
   * Given a fresh save where the squad has not played together yet
   * When the chemistry card renders
   * Then it shows neutral badges and the empty-state guidance without fake percentages
   */
  it("renders a clean neutral state for an unplayed save", async () => {
    const empty: TeamChemistryView = {
      cohesion_factor: 1.0,
      familiarity_factor: 1.0,
      formation: "4-4-2",
      tactical_familiarity: {
        formation: "",
        familiarity: 0,
        matches_together: 0,
      },
      advice: [],
      top_pairs: [],
    };
    mockedGetTeamChemistry.mockResolvedValueOnce(empty);

    render(<SquadChemistryCard refreshKey="t1|4-4-2||2031-07-01" />);

    const region = await screen.findByRole("region", { name: "Team Chemistry" });
    expect(within(region).getByText("4-4-2")).toBeInTheDocument();
    expect(within(region).getAllByText("Neutral")).toHaveLength(2);
    expect(
      within(region).getByText(
        "Play matches together to build on-pitch understanding and tactical familiarity.",
      ),
    ).toBeInTheDocument();
    expect(within(region).queryByRole("progressbar")).toBeNull();
  });

  /**
   * Given a side with built partnerships, familiarity, and assistant advice
   * When the chemistry card renders
   * Then cohesion delta, tactical familiarity bar, key partnerships, and advice notes are shown
   */
  it("renders cohesion, familiarity, top partnerships, and assistant notes", async () => {
    const populated: TeamChemistryView = {
      cohesion_factor: 1.024,
      familiarity_factor: 0.99,
      formation: "4-3-3",
      tactical_familiarity: {
        formation: "4-3-3",
        familiarity: 35,
        matches_together: 4,
      },
      top_pairs: [
        {
          player_a_id: "p1",
          player_a_name: "J. Smith",
          player_b_id: "p2",
          player_b_name: "A. Reid",
          bond: 58,
          shared_minutes: 720,
        },
        {
          player_a_id: "p3",
          player_a_name: "M. Cole",
          player_b_id: "p4",
          player_b_name: "L. Grant",
          bond: -25,
          shared_minutes: 270,
        },
      ],
      advice: [
        {
          kind: "StrongPartnership",
          player_ids: ["p1", "p2"],
          player_names: ["J. Smith", "A. Reid"],
          value: 58,
        },
        {
          kind: "WeakPartnership",
          player_ids: ["p3", "p4"],
          player_names: ["M. Cole", "L. Grant"],
          value: -25,
        },
        {
          kind: "UnfamiliarShape",
          player_ids: [],
          player_names: [],
          value: 35,
        },
      ],
    };
    mockedGetTeamChemistry.mockResolvedValueOnce(populated);

    render(<SquadChemistryCard refreshKey="t1|4-3-3|p1,p2|2031-10-01" />);

    const region = await screen.findByRole("region", { name: "Team Chemistry" });
    expect(within(region).getByText("+2.4%")).toBeInTheDocument();
    expect(
      within(region).getByRole("progressbar", { name: "Tactical Familiarity" }),
    ).toHaveAttribute("aria-valuenow", "35");
    expect(within(region).getByText("4 matches together")).toBeInTheDocument();
    expect(within(region).getByText("J. Smith — A. Reid")).toBeInTheDocument();
    expect(within(region).getByText("+58")).toBeInTheDocument();
    expect(within(region).getByText("720 min")).toBeInTheDocument();
    expect(within(region).getByText("Strong partnership")).toBeInTheDocument();
    expect(within(region).getByText("Weak partnership")).toBeInTheDocument();
    expect(within(region).getByText("Unfamiliar shape")).toBeInTheDocument();
    expect(
      within(region).getByText("The squad is still learning 4-3-3 (35%)."),
    ).toBeInTheDocument();
  });

  /**
   * Given an unemployed manager (backend answers null) or an IPC error
   * When the chemistry card mounts
   * Then nothing is rendered
   */
  it("renders nothing when backend returns null or fails", async () => {
    mockedGetTeamChemistry.mockResolvedValueOnce(null);
    const { container, rerender } = render(<SquadChemistryCard refreshKey="none" />);

    await waitFor(() => {
      expect(mockedGetTeamChemistry).toHaveBeenCalledTimes(1);
    });
    expect(container).toBeEmptyDOMElement();

    mockedGetTeamChemistry.mockRejectedValueOnce("be.error.noActiveGameSession");
    rerender(<SquadChemistryCard refreshKey="error" />);

    await waitFor(() => {
      expect(mockedGetTeamChemistry).toHaveBeenCalledTimes(2);
    });
    expect(container).toBeEmptyDOMElement();
  });
});
