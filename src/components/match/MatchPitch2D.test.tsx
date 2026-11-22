import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import MatchPitch2D from "./MatchPitch2D";

vi.mock("react-i18next", () => ({ useTranslation: () => ({ t: (key: string) => key }) }));

describe("MatchPitch2D", () => {
  it("shows the live pitch, ball, shot outcomes and xG accessibly", () => {
    render(
      <MatchPitch2D
        frame={{
          minute: 24,
          phase: "FirstHalf",
          ball: { x: 55, y: 50, owner_id: "p1", carrier_side: "Home", in_flight: false },
          players: [{ player_id: "p1", side: "Home", position: "Forward", x: 75, y: 35, has_ball: true }],
          last_event_label: null,
        }}
        shots={[{ minute: 18, side: "Home", shooter_id: "p1", x: 80, y: 50, xg: 0.32, outcome: "Saved", body_part: "Foot", assisted_by: null }]}
        homeXg={0.32}
        awayXg={0}
        homeName="Home FC"
        awayName="Away FC"
      />,
    );
    expect(screen.getByRole("img", { name: "match.pitch" })).toBeInTheDocument();
    expect(screen.getByText("match.shots: 1")).toBeInTheDocument();
    expect(screen.getByText("Home FC: 0.32 xG")).toBeInTheDocument();
  });
});
