import { act, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { TransferInstallmentsSummaryView } from "../../store/generated/TransferInstallmentsSummaryView";
import { getTransferInstallments } from "../../services/financeService";
import { formatVal } from "../../lib/helpers";
import { TransferInstallmentsCard } from "./TransferInstallmentsCard";

vi.mock("../../services/financeService", () => ({ getTransferInstallments: vi.fn() }));
vi.mock("react-i18next", () => ({
  initReactI18next: { type: "3rdParty", init: () => {} },
  useTranslation: () => ({
    t: (key: string, options?: { date?: string }) =>
      options?.date ? `${key}: ${options.date}` : key,
  }),
}));
const fetchInstallments = vi.mocked(getTransferInstallments);
const summary: TransferInstallmentsSummaryView = {
  outstanding_owed: 1000000,
  outstanding_receivable: 2000000,
  installments: [
    {
      id: "1",
      direction: "Payable",
      counterparty_team_id: "a",
      counterparty_team_name: "Club A",
      player_id: "p1",
      player_name: "Player One",
      amount: 1000000,
      due_date: "2030-01-01",
      settled: false,
    },
    {
      id: "2",
      direction: "Receivable",
      counterparty_team_id: "b",
      counterparty_team_name: "Club B",
      player_id: "p2",
      player_name: "Player Two",
      amount: 2000000,
      due_date: "2030-02-01",
      settled: true,
    },
  ],
};
beforeEach(() => {
  fetchInstallments.mockReset();
});
describe("TransferInstallmentsCard", () => {
  it("shows payables, receivables, totals and statuses", async () => {
    fetchInstallments.mockResolvedValue(summary);
    render(<TransferInstallmentsCard refreshKey="a" />);
    expect(await screen.findByText("Player One")).toBeInTheDocument();
    for (const text of [
      "Player Two",
      "Club A",
      "Club B",
      "finances.installments.payable",
      "finances.installments.receivable",
      "finances.installments.upcoming",
      "finances.installments.settled",
      "finances.installments.dueDate: 2030-01-01",
      "finances.installments.outstandingOwed",
      "finances.installments.outstandingReceivable",
    ])
      expect(screen.getByText(text)).toBeInTheDocument();
    expect(screen.getAllByText(formatVal(1000000))).toHaveLength(2);
    expect(screen.getAllByText(formatVal(2000000))).toHaveLength(2);
  });
  it("shows an empty schedule", async () => {
    fetchInstallments.mockResolvedValue({ ...summary, installments: [] });
    render(<TransferInstallmentsCard refreshKey="a" />);
    expect(await screen.findByText("finances.installments.empty")).toBeInTheDocument();
  });
  it.each(["unemployed", "error"])("renders nothing for %s", async (mode) => {
    if (mode === "error") fetchInstallments.mockRejectedValue(new Error("failed"));
    else fetchInstallments.mockResolvedValue(null);
    const { container } = render(<TransferInstallmentsCard refreshKey="a" />);
    await act(async () => {
      await Promise.resolve();
    });
    expect(fetchInstallments).toHaveBeenCalled();
    expect(container).toBeEmptyDOMElement();
  });
  it("hides old data and ignores out-of-order responses", async () => {
    let resolveOld!: (value: TransferInstallmentsSummaryView | null) => void;
    let resolveNew!: (value: TransferInstallmentsSummaryView | null) => void;
    fetchInstallments
      .mockResolvedValueOnce(summary)
      .mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            resolveOld = resolve;
          }),
      )
      .mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            resolveNew = resolve;
          }),
      );
    const { rerender } = render(<TransferInstallmentsCard refreshKey="a" />);
    expect(await screen.findByText("Player One")).toBeInTheDocument();
    rerender(<TransferInstallmentsCard refreshKey="b" />);
    expect(screen.queryByText("Player One")).not.toBeInTheDocument();
    rerender(<TransferInstallmentsCard refreshKey="c" />);
    await act(async () => resolveNew({ ...summary, installments: [] }));
    expect(screen.getByText("finances.installments.empty")).toBeInTheDocument();
    await act(async () => resolveOld(summary));
    expect(screen.queryByText("Player One")).not.toBeInTheDocument();
  });
});
