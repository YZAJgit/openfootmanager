import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ActiveLoanData, GameStateData } from "../../store/gameStore";
import { getPlayerTransferTerms, recallLoan } from "../../services/transfersService";
import PlayerProfileLoanStatusBanner from "./PlayerProfileLoanStatusBanner";

vi.mock("../../services/transfersService", () => ({
  getPlayerTransferTerms: vi.fn(),
  recallLoan: vi.fn(),
}));
vi.mock("../../utils/backendI18n", () => ({ resolveBackendError: () => "Recall failed" }));

const terms = vi.mocked(getPlayerTransferTerms);
const recall = vi.mocked(recallLoan);
const game = {} as GameStateData;
const loan = {
  parent_team_id: "parent",
  loan_team_id: "loan",
  end_date: "2026-06-01",
  wage_contribution_pct: 50,
  buy_option_fee: null,
} as ActiveLoanData;
const renderBanner = (onGameUpdate = vi.fn()) => {
  render(
    <PlayerProfileLoanStatusBanner
      playerId="p1"
      loan={loan}
      teams={[]}
      managerTeamId="parent"
      language="en"
      t={(key) => key}
      onGameUpdate={onGameUpdate}
    />,
  );
  return onGameUpdate;
};

beforeEach(() => {
  vi.resetAllMocks();
});

describe("loan recall", () => {
  it.each([false, undefined])("hides recall when recallable is %s", async (value) => {
    terms.mockResolvedValue({ loan_recallable: value } as Awaited<
      ReturnType<typeof getPlayerTransferTerms>
    >);
    renderBanner();
    await waitFor(() => expect(terms).toHaveBeenCalledWith("p1"));
    expect(screen.queryByRole("button", { name: "transfers.recallLoan" })).toBeNull();
  });

  it("disables recall while busy and updates the game", async () => {
    terms.mockResolvedValue({ loan_recallable: true } as Awaited<
      ReturnType<typeof getPlayerTransferTerms>
    >);
    let finish!: (value: GameStateData) => void;
    recall.mockReturnValue(
      new Promise((resolve) => {
        finish = resolve;
      }),
    );
    const update = renderBanner();
    fireEvent.click(await screen.findByRole("button", { name: "transfers.recallLoan" }));
    expect(screen.getByRole("button", { name: "transfers.recallingLoan" })).toBeDisabled();
    finish(game);
    await waitFor(() => expect(update).toHaveBeenCalledWith(game));
    expect(recall).toHaveBeenCalledWith("p1");
  });

  it("ignores a recall rejection after switching players", async () => {
    terms.mockResolvedValue({ loan_recallable: true } as Awaited<
      ReturnType<typeof getPlayerTransferTerms>
    >);
    let reject!: (error: Error) => void;
    recall.mockReturnValue(
      new Promise((_resolve, rejectPromise) => {
        reject = rejectPromise;
      }),
    );
    const props = {
      loan,
      teams: [],
      managerTeamId: "parent",
      language: "en",
      t: (key: string) => key,
      onGameUpdate: vi.fn(),
    };
    const { rerender } = render(<PlayerProfileLoanStatusBanner {...props} playerId="p1" />);
    fireEvent.click(await screen.findByRole("button", { name: "transfers.recallLoan" }));
    rerender(<PlayerProfileLoanStatusBanner {...props} playerId="p2" />);
    await act(async () => reject(new Error("failure")));
    expect(screen.queryByRole("alert")).toBeNull();
    expect(await screen.findByRole("button", { name: "transfers.recallLoan" })).toBeEnabled();
  });

  it("surfaces recall errors", async () => {
    terms.mockResolvedValue({ loan_recallable: true } as Awaited<
      ReturnType<typeof getPlayerTransferTerms>
    >);
    recall.mockRejectedValue(new Error("failure"));
    renderBanner();
    fireEvent.click(await screen.findByRole("button", { name: "transfers.recallLoan" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Recall failed");
  });
});
