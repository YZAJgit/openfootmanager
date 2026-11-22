import { useTranslation } from "react-i18next";
import type { PitchFrame2D } from "../../store/generated/PitchFrame2D";
import type { ShotMapEntry } from "../../store/generated/ShotMapEntry";

export default function MatchPitch2D({
  frame,
  shots,
  homeXg,
  awayXg,
  homeName,
  awayName,
}: {
  frame: PitchFrame2D | null;
  shots: ShotMapEntry[];
  homeXg: number;
  awayXg: number;
  homeName: string;
  awayName: string;
}) {
  const { t } = useTranslation();
  return (
    <div className="flex flex-col gap-3">
      <div
        className="flex justify-between gap-2 text-body tabular-nums"
        aria-label={t("match.expectedGoals")}
      >
        <span className="text-fg">
          {homeName}: {homeXg.toFixed(2)} xG
        </span>
        <span className="text-fg-muted">
          {t("match.shots")}: {shots.length}
        </span>
        <span className="text-fg">
          {awayName}: {awayXg.toFixed(2)} xG
        </span>
      </div>
      <svg
        viewBox="0 0 100 100"
        role="img"
        aria-label={t("match.pitch")}
        className="w-full max-h-96 rounded-md bg-positive-tint text-on-brand"
      >
        <rect x="2" y="2" width="96" height="96" fill="none" stroke="currentColor" strokeWidth="0.6" />
        <line x1="50" y1="2" x2="50" y2="98" stroke="currentColor" strokeWidth="0.4" />
        <circle cx="50" cy="50" r="9" fill="none" stroke="currentColor" strokeWidth="0.4" />
        {frame?.players.map((player) => (
          <circle
            key={player.player_id}
            cx={player.x}
            cy={player.y}
            r="1.7"
            fill={player.side === "Home" ? "var(--color-selected-fill)" : "var(--color-info-solid)"}
            aria-label={player.position}
          />
        ))}
        {frame && (
          <circle
            cx={frame.ball.x}
            cy={frame.ball.y}
            r="1.2"
            fill="var(--color-on-brand)"
            stroke="var(--color-fg)"
            strokeWidth="0.4"
          />
        )}
        {shots.map((shot, index) => (
          <circle
            key={`${shot.minute}-${shot.shooter_id}-${index}`}
            cx={shot.x}
            cy={shot.y}
            r="2.2"
            fill={shot.side === "Home" ? "var(--color-selected-fill)" : "var(--color-info-solid)"}
            stroke={shot.outcome === "Goal" ? "var(--color-positive-solid)" : "var(--color-page)"}
            strokeWidth={shot.outcome === "Goal" ? "1" : "0.5"}
            aria-label={`${t("match.shots")} ${shot.minute}`}
          >
            <title>{`${shot.minute}' · ${shot.xg.toFixed(2)} xG`}</title>
          </circle>
        ))}
      </svg>
    </div>
  );
}
