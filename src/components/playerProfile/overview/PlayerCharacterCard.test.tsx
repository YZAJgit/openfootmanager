import { render, screen, waitFor, within } from "@testing-library/react";
import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";

import { i18nReady } from "../../../i18n";
import { getPlayerCharacter } from "../../../services/chemistryService";
import type { PlayerCharacterView } from "../../../store/generated/PlayerCharacterView";
import PlayerCharacterCard from "./PlayerCharacterCard";

vi.mock("../../../services/chemistryService", () => ({
  getPlayerCharacter: vi.fn(),
}));

const mockedGetPlayerCharacter = vi.mocked(getPlayerCharacter);

beforeAll(async () => {
  await i18nReady;
});

beforeEach(() => {
  mockedGetPlayerCharacter.mockReset();
});

const KNOWN_VIEW: PlayerCharacterView = {
  known: true,
  labels: ["ModelProfessional", "BigGamePlayer"],
  profile: {
    professionalism: 88,
    ambition: 74,
    loyalty: 68,
    consistency: 79,
    big_matches: 85,
    pressure_handling: 76,
    injury_proneness: 28,
    adaptability: 64,
  },
};

describe("PlayerCharacterCard", () => {
  it("refreshes knowledge for the same player", async () => {
    mockedGetPlayerCharacter.mockResolvedValueOnce({ known: false, labels: [], profile: null });
    mockedGetPlayerCharacter.mockResolvedValueOnce(KNOWN_VIEW);
    const { rerender } = render(<PlayerCharacterCard playerId="p1" refreshKey="unknown" />);
    await waitFor(() => expect(mockedGetPlayerCharacter).toHaveBeenCalledTimes(1));
    rerender(<PlayerCharacterCard playerId="p1" refreshKey="known" />);
    expect(await screen.findByText("Model professional")).toBeInTheDocument();
    expect(mockedGetPlayerCharacter).toHaveBeenCalledTimes(2);
  });
  /**
   * Given a known player with labels and an assessed profile
   * When the card loads
   * Then it renders the Character region with each label badge and all eight trait values
   */
  it("shows labels and all eight traits for a known player", async () => {
    mockedGetPlayerCharacter.mockResolvedValueOnce(KNOWN_VIEW);

    render(<PlayerCharacterCard playerId="p1" />);

    const card = await screen.findByRole("region", { name: "Character" });
    expect(within(card).getByText("Model professional")).toBeInTheDocument();
    expect(within(card).getByText("Big-game player")).toBeInTheDocument();
    expect(within(card).getByText("Professionalism")).toBeInTheDocument();
    expect(within(card).getByText("88")).toBeInTheDocument();
    expect(within(card).getByText("Adaptability")).toBeInTheDocument();
    expect(within(card).getByText("64")).toBeInTheDocument();
  });

  /**
   * Given an unknown player whose character is withheld by the backend
   * When the card loads
   * Then nothing is rendered and no trait leaks into the document
   */
  it("renders nothing for an unknown player", async () => {
    mockedGetPlayerCharacter.mockResolvedValueOnce({
      known: false,
      labels: [],
      profile: null,
    });

    const { container } = render(<PlayerCharacterCard playerId="stranger" />);

    await waitFor(() => {
      expect(mockedGetPlayerCharacter).toHaveBeenCalledWith("stranger");
    });
    expect(container).toBeEmptyDOMElement();
  });

  /**
   * Given a player change while the previous request is still in flight
   * When the stale request resolves after switching players
   * Then the stale player's character is ignored and only the current player's renders
   */
  it("cancels stale requests when playerId changes", async () => {
    let resolveFirst: ((value: PlayerCharacterView | null) => void) | undefined;
    mockedGetPlayerCharacter.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          resolveFirst = resolve;
        }),
    );
    mockedGetPlayerCharacter.mockResolvedValueOnce({
      known: true,
      labels: ["Loyal"],
      profile: {
        professionalism: 60,
        ambition: 55,
        loyalty: 91,
        consistency: 62,
        big_matches: 58,
        pressure_handling: 61,
        injury_proneness: 40,
        adaptability: 57,
      },
    });

    const { rerender } = render(<PlayerCharacterCard playerId="p1" />);
    rerender(<PlayerCharacterCard playerId="p2" />);

    const card = await screen.findByRole("region", { name: "Character" });
    expect(within(card).getByText("Loyal")).toBeInTheDocument();
    expect(within(card).getByText("91")).toBeInTheDocument();

    resolveFirst?.(KNOWN_VIEW);
    await waitFor(() => {
      expect(within(card).queryByText("Model professional")).toBeNull();
      expect(within(card).getByText("Loyal")).toBeInTheDocument();
    });
  });

  /**
   * Given a failing backend request
   * When the card mounts
   * Then the failure is handled silently without throwing or rendering empty chrome
   */
  it("renders nothing when the backend request fails", async () => {
    mockedGetPlayerCharacter.mockRejectedValueOnce("be.error.noActiveGameSession");

    const { container } = render(<PlayerCharacterCard playerId="p1" />);

    await waitFor(() => {
      expect(mockedGetPlayerCharacter).toHaveBeenCalledWith("p1");
    });
    expect(container).toBeEmptyDOMElement();
  });
});
