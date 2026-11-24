import { describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import {
  decideScholarshipAction,
  getYouthAcademyOverview,
  recallYouthLoanBreach,
} from "./youthAcademyService";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn().mockResolvedValue(null) }));
vi.mock("./careerInvoke", () => ({ invokeForCareer: vi.fn().mockResolvedValue(null) }));
import { invokeForCareer } from "./careerInvoke";

describe("youth academy service", () => {
  it("reads overview and sends career-scoped writes", async () => {
    await getYouthAcademyOverview();
    await decideScholarshipAction("p", true);
    await recallYouthLoanBreach("p");
    expect(invoke).toHaveBeenCalledWith("get_youth_academy_overview");
    expect(invokeForCareer).toHaveBeenCalledWith("decide_scholarship_action", {
      playerId: "p",
      offerPro: true,
    });
    expect(invokeForCareer).toHaveBeenCalledWith("recall_youth_loan_breach", { playerId: "p" });
  });
});
