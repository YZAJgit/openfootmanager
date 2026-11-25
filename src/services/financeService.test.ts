import { describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { getTransferInstallments } from "./financeService";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
describe("getTransferInstallments", () => {
  it("invokes the transfer installments command", async () => {
    vi.mocked(invoke).mockResolvedValueOnce(null);
    expect(await getTransferInstallments()).toBeNull();
    expect(invoke).toHaveBeenCalledWith("get_transfer_installments");
  });
});
