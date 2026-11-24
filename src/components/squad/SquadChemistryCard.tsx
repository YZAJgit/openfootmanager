import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

import { getTeamChemistry } from "../../services/chemistryService";
import type { AdviceKind } from "../../store/generated/AdviceKind";
import type { AssistantAdviceView } from "../../store/generated/AssistantAdviceView";
import type { TeamChemistryView } from "../../store/generated/TeamChemistryView";
import { Badge, Card, ProgressBar } from "../ui";

interface SquadChemistryCardProps {
  /** Changes when the lineup, formation, club, or date changes so the card refreshes. */
  refreshKey: string;
}

type ChemistryLoadState =
  | { status: "loading" }
  | { status: "loaded"; refreshKey: string; data: TeamChemistryView | null }
  | { status: "error" };

function formatSigned(value: number): string {
  return value > 0 ? `+${value}` : String(value);
}

function formatCohesionDelta(
  factor: number,
  neutralLabel: string,
): {
  text: string;
  variant: "success" | "danger" | "neutral";
} {
  const pct = Math.round((factor - 1) * 1000) / 10;
  if (pct > 0) {
    return { text: `+${pct.toFixed(1)}%`, variant: "success" };
  }
  if (pct < 0) {
    return { text: `${pct.toFixed(1)}%`, variant: "danger" };
  }
  return { text: neutralLabel, variant: "neutral" };
}

function adviceBadgeVariant(kind: AdviceKind): "success" | "danger" | "accent" {
  switch (kind) {
    case "StrongPartnership":
      return "success";
    case "WeakPartnership":
      return "danger";
    case "UnfamiliarShape":
      return "accent";
  }
}

function adviceKindLabel(kind: AdviceKind, t: (key: string) => string): string {
  switch (kind) {
    case "StrongPartnership":
      return t("chemistry.adviceKinds.StrongPartnership");
    case "WeakPartnership":
      return t("chemistry.adviceKinds.WeakPartnership");
    case "UnfamiliarShape":
      return t("chemistry.adviceKinds.UnfamiliarShape");
  }
}

function adviceDescription(
  item: AssistantAdviceView,
  formation: string,
  t: (key: string, options?: Record<string, string | number>) => string,
): string {
  const players = item.player_names.join(" & ");
  switch (item.kind) {
    case "StrongPartnership":
      return t("chemistry.adviceText.StrongPartnership", {
        players,
        value: formatSigned(item.value),
      });
    case "WeakPartnership":
      return t("chemistry.adviceText.WeakPartnership", {
        players,
        value: formatSigned(item.value),
      });
    case "UnfamiliarShape":
      return t("chemistry.adviceText.UnfamiliarShape", {
        formation,
        value: item.value,
      });
  }
}

function bondVariant(bond: number): "success" | "danger" | "neutral" {
  if (bond > 0) return "success";
  if (bond < 0) return "danger";
  return "neutral";
}

/**
 * Compact summary of on-pitch squad cohesion, formation familiarity, key partnerships,
 * and assistant notes for the Squad screen.
 */
export default function SquadChemistryCard({ refreshKey }: SquadChemistryCardProps) {
  const { t } = useTranslation();
  const [state, setState] = useState<ChemistryLoadState>({ status: "loading" });

  useEffect(() => {
    let cancelled = false;

    getTeamChemistry()
      .then((data) => {
        if (!cancelled) {
          setState({ status: "loaded", refreshKey, data });
        }
      })
      .catch(() => {
        if (!cancelled) {
          setState({ status: "error" });
        }
      });

    return () => {
      cancelled = true;
    };
  }, [refreshKey]);

  if (state.status !== "loaded" || state.refreshKey !== refreshKey || !state.data) {
    return null;
  }

  const view = state.data;
  const cohesion = formatCohesionDelta(view.cohesion_factor, t("chemistry.neutral"));
  const sameFormation = view.tactical_familiarity.formation === view.formation;
  const hasPlayed = view.tactical_familiarity.matches_together > 0;
  const familiarityPct = hasPlayed
    ? Math.round(view.tactical_familiarity.familiarity * (sameFormation ? 1 : 0.4))
    : 0;
  const hasDetails = view.top_pairs.length > 0 || view.advice.length > 0;

  return (
    <section aria-label={t("chemistry.title")}>
      <Card>
        <div className="flex flex-wrap items-center justify-between gap-3 border-b border-line px-4 py-3">
          <div className="flex flex-wrap items-center gap-2">
            <h3 className="font-heading text-body font-bold uppercase tracking-label text-fg">
              {t("chemistry.title")}
            </h3>
            <Badge variant="outline" size="sm">
              {view.formation}
            </Badge>
          </div>
          <div className="flex flex-wrap items-center gap-4 text-caption">
            <div className="flex items-center gap-1.5">
              <span className="text-fg-muted">{t("chemistry.cohesion")}</span>
              <Badge variant={cohesion.variant} size="sm">
                {cohesion.text}
              </Badge>
            </div>
            <div className="flex items-center gap-2">
              <span className="text-fg-muted">{t("chemistry.tacticalFamiliarity")}</span>
              {hasPlayed ? (
                <div className="flex items-center gap-2">
                  <div className="w-24">
                    <ProgressBar
                      value={familiarityPct}
                      label={t("chemistry.tacticalFamiliarity")}
                      size="sm"
                      showLabel
                    />
                  </div>
                  {sameFormation && (
                    <span className="text-fg-muted tabular-nums">
                      {t("chemistry.matchesTogether", {
                        count: view.tactical_familiarity.matches_together,
                      })}
                    </span>
                  )}
                </div>
              ) : (
                <Badge variant="neutral" size="sm">
                  {t("chemistry.neutral")}
                </Badge>
              )}
            </div>
          </div>
        </div>

        {hasDetails ? (
          <div className="grid grid-cols-1 gap-4 px-4 py-3 md:grid-cols-2">
            {view.top_pairs.length > 0 ? (
              <div className="flex min-w-0 flex-col gap-2">
                <h4 className="font-heading text-caption font-bold uppercase tracking-label text-fg-muted">
                  {t("chemistry.topPartnerships")}
                </h4>
                {/* biome-ignore lint/a11y/noRedundantRoles: WebKit drops a list's semantics under list-style: none, so the role is restated. */}
                <ul role="list" className="m-0 flex list-none flex-col gap-1.5 p-0">
                  {view.top_pairs.map((pair) => (
                    <li
                      key={`${pair.player_a_id}-${pair.player_b_id}`}
                      className="flex items-center justify-between gap-2 rounded-md border border-line bg-sunken px-2.5 py-1.5 text-caption"
                    >
                      <span className="truncate font-medium text-fg">
                        {pair.player_a_name} — {pair.player_b_name}
                      </span>
                      <span className="flex shrink-0 items-center gap-2 tabular-nums">
                        <span className="text-fg-muted">
                          {t("chemistry.sharedMinutes", { count: pair.shared_minutes })}
                        </span>
                        <Badge variant={bondVariant(pair.bond)} size="sm">
                          {formatSigned(pair.bond)}
                        </Badge>
                      </span>
                    </li>
                  ))}
                </ul>
              </div>
            ) : null}

            {view.advice.length > 0 ? (
              <div className="flex min-w-0 flex-col gap-2">
                <h4 className="font-heading text-caption font-bold uppercase tracking-label text-fg-muted">
                  {t("chemistry.assistantAdvice")}
                </h4>
                {/* biome-ignore lint/a11y/noRedundantRoles: WebKit drops a list's semantics under list-style: none, so the role is restated. */}
                <ul role="list" className="m-0 flex list-none flex-col gap-1.5 p-0">
                  {view.advice.map((item, index) => (
                    <li
                      key={`${item.kind}-${item.player_ids.join("-")}-${index}`}
                      className="flex items-start gap-2 rounded-md border border-line bg-sunken px-2.5 py-1.5 text-caption"
                    >
                      <Badge variant={adviceBadgeVariant(item.kind)} size="sm" className="shrink-0">
                        {adviceKindLabel(item.kind, t)}
                      </Badge>
                      <span className="text-fg">{adviceDescription(item, view.formation, t)}</span>
                    </li>
                  ))}
                </ul>
              </div>
            ) : null}
          </div>
        ) : (
          <p className="px-4 py-2.5 text-caption text-fg-muted">{t("chemistry.noDataYet")}</p>
        )}
      </Card>
    </section>
  );
}
