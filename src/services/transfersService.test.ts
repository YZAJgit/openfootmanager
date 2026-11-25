import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useGameStore } from "../store/gameStore";
import {
  getPlayerTransferTerms,
  makeStructuredTransferBid,
  previewTransferBidFinancialImpact,
  recallLoan,
} from "./transfersService";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

describe("transfer service commands", () => {
  beforeEach(() => {
    vi.mocked(invoke).mockReset();
    vi.mocked(invoke).mockResolvedValue(undefined);
    useGameStore.setState({ gameState: null });
  });

  it("requests player transfer terms", async () => {
    await getPlayerTransferTerms("p1");
    expect(invoke).toHaveBeenCalledWith("get_player_transfer_terms", { playerId: "p1" });
  });

  it("sends the structured bid and career token", async () => {
    await makeStructuredTransferBid("p1", 1000, 400, 2, 90);
    expect(invoke).toHaveBeenCalledWith("make_structured_transfer_bid", {
      playerId: "p1",
      totalFee: 1000,
      upfrontFee: 400,
      trancheCount: 2,
      intervalDays: 90,
      careerToken: 0,
    });
  });

  it("recalls a loan with the career token", async () => {
    await recallLoan("p1");
    expect(invoke).toHaveBeenCalledWith("recall_loan", { playerId: "p1", careerToken: 0 });
  });

  it("previews an outright bid without an upfront fee", async () => {
    await previewTransferBidFinancialImpact("p1", 1000);
    expect(invoke).toHaveBeenCalledWith("preview_transfer_bid_financial_impact", {
      playerId: "p1",
      fee: 1000,
    });
  });

  it("previews a structured bid with its upfront fee", async () => {
    await previewTransferBidFinancialImpact("p1", 1000, 0);
    expect(invoke).toHaveBeenCalledWith("preview_transfer_bid_financial_impact", {
      playerId: "p1",
      fee: 1000,
      upfrontFee: 0,
    });
  });
});
