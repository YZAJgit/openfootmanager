import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

import { getPlayerCharacter } from "../../../services/chemistryService";
import type { CharacterLabel } from "../../../store/generated/CharacterLabel";
import type { CharacterProfile } from "../../../store/generated/CharacterProfile";
import type { PlayerCharacterView } from "../../../store/generated/PlayerCharacterView";
import { Badge } from "../../ui";
import { OverviewCard } from "./OverviewCard";

interface PlayerCharacterCardProps {
  playerId: string;
  refreshKey?: string;
}

type CharacterLoadState =
  | { status: "loading"; playerId: string; refreshKey?: string }
  | { status: "loaded"; playerId: string; refreshKey?: string; data: PlayerCharacterView | null }
  | { status: "error"; playerId: string; refreshKey?: string };

type TraitKey = keyof CharacterProfile;

const TRAIT_ORDER: readonly TraitKey[] = [
  "professionalism",
  "ambition",
  "loyalty",
  "consistency",
  "big_matches",
  "pressure_handling",
  "injury_proneness",
  "adaptability",
];

function badgeVariantFor(label: CharacterLabel): "success" | "accent" | "danger" | "neutral" {
  switch (label) {
    case "ModelProfessional":
    case "Loyal":
    case "Consistent":
      return "success";
    case "Driven":
    case "BigGamePlayer":
      return "accent";
    case "Temperamental":
    case "InjuryProne":
      return "danger";
    case "Balanced":
      return "neutral";
  }
}

function labelText(label: CharacterLabel, t: (key: string) => string): string {
  switch (label) {
    case "ModelProfessional":
      return t("character.labels.ModelProfessional");
    case "Driven":
      return t("character.labels.Driven");
    case "Loyal":
      return t("character.labels.Loyal");
    case "BigGamePlayer":
      return t("character.labels.BigGamePlayer");
    case "Consistent":
      return t("character.labels.Consistent");
    case "Temperamental":
      return t("character.labels.Temperamental");
    case "InjuryProne":
      return t("character.labels.InjuryProne");
    case "Balanced":
      return t("character.labels.Balanced");
  }
}

function traitText(trait: TraitKey, t: (key: string) => string): string {
  switch (trait) {
    case "professionalism":
      return t("character.traits.professionalism");
    case "ambition":
      return t("character.traits.ambition");
    case "loyalty":
      return t("character.traits.loyalty");
    case "consistency":
      return t("character.traits.consistency");
    case "big_matches":
      return t("character.traits.big_matches");
    case "pressure_handling":
      return t("character.traits.pressure_handling");
    case "injury_proneness":
      return t("character.traits.injury_proneness");
    case "adaptability":
      return t("character.traits.adaptability");
  }
}

/**
 * Character summary card for a player's profile Overview.
 * Renders label badges and the 8 character traits only when the backend marks the player as known;
 * unknown players, loading states, and failed loads render nothing so hidden traits never leak.
 */
export default function PlayerCharacterCard({ playerId, refreshKey }: PlayerCharacterCardProps) {
  const { t } = useTranslation();
  const [state, setState] = useState<CharacterLoadState>({
    status: "loading",
    playerId,
    refreshKey,
  });

  useEffect(() => {
    let cancelled = false;
    setState({ status: "loading", playerId, refreshKey });

    getPlayerCharacter(playerId)
      .then((data) => {
        if (!cancelled) {
          setState({ status: "loaded", playerId, refreshKey, data });
        }
      })
      .catch(() => {
        if (!cancelled) {
          setState({ status: "error", playerId, refreshKey });
        }
      });

    return () => {
      cancelled = true;
    };
  }, [playerId, refreshKey]);

  if (state.status !== "loaded" || state.playerId !== playerId || state.refreshKey !== refreshKey) {
    return null;
  }

  const view = state.data;
  if (!view?.known || !view.profile) {
    return null;
  }

  const { labels, profile } = view;

  return (
    <OverviewCard title={t("character.title")}>
      {labels.length > 0 ? (
        <ul
          // biome-ignore lint/a11y/noRedundantRoles: WebKit drops list semantics under list-style: none.
          role="list"
          aria-label={t("character.labelsAria")}
          className="m-0 flex list-none flex-wrap gap-1.5 border-b border-line px-4 py-2.5"
        >
          {labels.map((label) => (
            <li key={label}>
              <Badge variant={badgeVariantFor(label)} size="sm">
                {labelText(label, t)}
              </Badge>
            </li>
          ))}
        </ul>
      ) : null}
      <dl className="grid grid-cols-2 gap-x-4 gap-y-1.5 px-4 py-3 text-caption">
        {TRAIT_ORDER.map((trait) => (
          <div key={trait} className="flex items-center justify-between gap-2">
            <dt className="truncate text-fg-muted">{traitText(trait, t)}</dt>
            <dd className="font-semibold tabular-nums text-fg">{profile[trait]}</dd>
          </div>
        ))}
      </dl>
    </OverviewCard>
  );
}
