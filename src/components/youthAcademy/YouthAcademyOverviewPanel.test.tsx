import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import * as service from "../../services/youthAcademyService";
import type { GameStateData } from "../../store/gameStore";
import type { YouthAcademyOverview } from "../../store/generated/YouthAcademyOverview";
import YouthAcademyOverviewPanel from "./YouthAcademyOverviewPanel";

vi.mock("../../services/youthAcademyService", () => ({
  getYouthAcademyOverview: vi.fn(),
  decideScholarshipAction: vi.fn(),
  recallYouthLoanBreach: vi.fn(),
}));
vi.mock("../../utils/backendI18n", () => ({ resolveBackendError: () => "resolved error" }));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string, options?: { count?: number }) => `${key}${options?.count ?? ""}`,
  }),
}));
const overview: YouthAcademyOverview = {
  academy_tier: "CategoryOne",
  home_grown: {
    club_trained_count: 1,
    club_trained_quota: 4,
    club_trained_shortfall: 3,
    nation_trained_count: 8,
    nation_trained_quota: 8,
    nation_trained_shortfall: 0,
  },
  scholars: [
    {
      player_id: "s",
      full_name: "Scholar",
      age: 17,
      joined_on: null,
      scholarship_deadline: "2027-01-01",
      decision_due: true,
    },
  ],
  loaned_youth: [
    {
      player_id: "l",
      full_name: "Loan",
      loan_team_id: "t",
      loan_team_name: "Club",
      guaranteed_minutes_pct: 50,
      required_minutes: 100,
      actual_minutes: 20,
      available_minutes: 200,
      is_breach: true,
      loan_recallable: true,
    },
  ],
};
const game = {} as GameStateData;
beforeEach(() => {
  vi.resetAllMocks();
  vi.mocked(service.getYouthAcademyOverview).mockResolvedValue(overview);
});
describe("YouthAcademyOverviewPanel", () => {
  it("shows tier, quotas, shortfall and loan breach", async () => {
    render(<YouthAcademyOverviewPanel refreshKey="a" onGameUpdate={vi.fn()} />);
    expect(await screen.findByText("youthAcademy.tierCategoryOne")).toBeInTheDocument();
    expect(screen.getByText(/youthAcademy.clubTrained: 1 \/ 4/)).toBeInTheDocument();
    expect(screen.getByText("youthAcademy.shortfall3")).toBeInTheDocument();
    expect(
      screen.getByText(/youthAcademy.guaranteedMinutes: 20 \/ 100 \(50%\)/),
    ).toBeInTheDocument();
    expect(screen.getByText("youthAcademy.minutesBreach")).toBeInTheDocument();
  });
  it.each([
    ["offerPro", true],
    ["releaseScholar", false],
  ] as const)("submits %s and refreshes", async (label, decision) => {
    const update = vi.fn();
    let finish!: (value: GameStateData) => void;
    vi.mocked(service.decideScholarshipAction).mockReturnValue(
      new Promise((resolve) => {
        finish = resolve;
      }),
    );
    render(<YouthAcademyOverviewPanel refreshKey="a" onGameUpdate={update} />);
    fireEvent.click(await screen.findByText(`youthAcademy.${label}`));
    expect(service.decideScholarshipAction).toHaveBeenCalledWith("s", decision);
    expect(screen.getByText("youthAcademy.offerPro")).toBeDisabled();
    await act(async () => finish(game));
    await waitFor(() => expect(service.getYouthAcademyOverview).toHaveBeenCalledTimes(2));
    expect(update).toHaveBeenCalledWith(game);
  });
  it("recalls a breach and refreshes", async () => {
    const update = vi.fn();
    vi.mocked(service.recallYouthLoanBreach).mockResolvedValue(game);
    render(<YouthAcademyOverviewPanel refreshKey="a" onGameUpdate={update} />);
    fireEvent.click(await screen.findByText("youthAcademy.recallBreach"));
    await waitFor(() => expect(service.getYouthAcademyOverview).toHaveBeenCalledTimes(2));
    expect(service.recallYouthLoanBreach).toHaveBeenCalledWith("l");
    expect(update).toHaveBeenCalledWith(game);
  });
  it.each(["scholar", "loan"])("shows resolved %s errors", async (kind) => {
    vi.mocked(service.decideScholarshipAction).mockRejectedValue(new Error("fail"));
    vi.mocked(service.recallYouthLoanBreach).mockRejectedValue(new Error("fail"));
    render(<YouthAcademyOverviewPanel refreshKey="a" onGameUpdate={vi.fn()} />);
    fireEvent.click(
      await screen.findByText(
        kind === "loan" ? "youthAcademy.recallBreach" : "youthAcademy.offerPro",
      ),
    );
    expect(await screen.findByRole("alert")).toHaveTextContent("resolved error");
  });
  it.each([
    [false, true],
    [true, false],
    [false, false],
  ])("hides recall without breach and permission", async (is_breach, loan_recallable) => {
    vi.mocked(service.getYouthAcademyOverview).mockResolvedValue({
      ...overview,
      loaned_youth: [{ ...overview.loaned_youth[0], is_breach, loan_recallable }],
    });
    render(<YouthAcademyOverviewPanel refreshKey="a" onGameUpdate={vi.fn()} />);
    await screen.findByText(/Loan/);
    expect(screen.queryByText("youthAcademy.recallBreach")).toBeNull();
  });
  it("handles unemployed and empty lists", async () => {
    vi.mocked(service.getYouthAcademyOverview)
      .mockResolvedValueOnce(null)
      .mockResolvedValueOnce({ ...overview, scholars: [], loaned_youth: [] });
    const { container, rerender } = render(<YouthAcademyOverviewPanel refreshKey="a" />);
    await waitFor(() => expect(service.getYouthAcademyOverview).toHaveBeenCalled());
    expect(container).toBeEmptyDOMElement();
    rerender(<YouthAcademyOverviewPanel refreshKey="b" />);
    expect(await screen.findByText("youthAcademy.noContracts")).toBeInTheDocument();
    expect(screen.getByText("youthAcademy.noLoans")).toBeInTheDocument();
  });
  it("hides previous data and ignores stale responses", async () => {
    let finish!: (value: YouthAcademyOverview | null) => void;
    vi.mocked(service.getYouthAcademyOverview)
      .mockResolvedValueOnce(overview)
      .mockReturnValueOnce(
        new Promise((resolve) => {
          finish = resolve;
        }),
      )
      .mockResolvedValueOnce({ ...overview, scholars: [] });
    const { rerender } = render(<YouthAcademyOverviewPanel refreshKey="a" />);
    await screen.findByText(/^Scholar,/);
    rerender(<YouthAcademyOverviewPanel refreshKey="b" />);
    expect(screen.queryByText(/^Scholar,/)).toBeNull();
    rerender(<YouthAcademyOverviewPanel refreshKey="c" />);
    await screen.findByText("youthAcademy.noContracts");
    await act(async () => finish(overview));
    expect(screen.queryByText(/^Scholar,/)).toBeNull();
  });
});
