import { useEffect, useRef, useState } from "react";
import { getPlayerTransferTerms, recallLoan } from "../../services/transfersService";
import { resolveBackendError } from "../../utils/backendI18n";
import type { GameStateData } from "../../store/gameStore";
import { ArrowRightLeft, BadgeEuro, CalendarDays, Percent } from "lucide-react";
import type { TOptions } from "i18next";

import { formatDate } from "../../lib/dateFormatting";
import { formatExactMoney } from "../../lib/helpers";
import type { ActiveLoanData, TeamData } from "../../store/gameStore";
import { Card, CardBody } from "../ui";
import { getPlayerTeamName } from "./PlayerProfile.helpers";

type TranslateFn = (key: string, options?: TOptions) => string;

interface PlayerProfileLoanStatusBannerProps {
  playerId?: string;
  onGameUpdate?: (game: GameStateData) => void;
  loan: ActiveLoanData;
  teams: TeamData[];
  managerTeamId: string | null;
  language: string;
  t: TranslateFn;
}

export default function PlayerProfileLoanStatusBanner({
  playerId,
  onGameUpdate,
  loan,
  teams,
  managerTeamId,
  language,
  t,
}: PlayerProfileLoanStatusBannerProps) {
  const [recallable, setRecallable] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const sessionRef = useRef(0);
  useEffect(() => {
    sessionRef.current += 1;
    setBusy(false);
    return () => {
      sessionRef.current += 1;
    };
  }, [playerId]);
  useEffect(() => {
    let cancelled = false;
    setRecallable(false);
    setError(null);
    if (playerId)
      getPlayerTransferTerms(playerId)
        .then((terms) => {
          if (!cancelled) setRecallable(terms?.loan_recallable === true);
        })
        .catch(() => {
          if (!cancelled) setRecallable(false);
        });
    return () => {
      cancelled = true;
    };
  }, [playerId, managerTeamId, loan.parent_team_id, loan.loan_team_id]);
  const handleRecall = async () => {
    if (!playerId || busy) return;
    const session = sessionRef.current;
    setBusy(true);
    setError(null);
    try {
      const game = await recallLoan(playerId);
      if (sessionRef.current === session) onGameUpdate?.(game);
    } catch (err) {
      if (sessionRef.current === session) setError(resolveBackendError(err));
    } finally {
      if (sessionRef.current === session) setBusy(false);
    }
  };
  const labels = {
    freeAgent: t("common.freeAgent"),
    unknown: t("common.unknown"),
  };
  const parentTeamName = getPlayerTeamName(teams, loan.parent_team_id, labels);
  const loanTeamName = getPlayerTeamName(teams, loan.loan_team_id, labels);
  const relationship =
    managerTeamId === loan.parent_team_id
      ? t("playerProfile.loanedOutTo", { team: loanTeamName })
      : managerTeamId === loan.loan_team_id
        ? t("playerProfile.loanedInFrom", { team: parentTeamName })
        : t("playerProfile.loanBetweenClubs", {
            parent: parentTeamName,
            loanTeam: loanTeamName,
          });

  return (
    <Card accent="primary" className="mb-5">
      <CardBody>
        <div className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
          <div className="flex items-center gap-3">
            <div className="flex h-10 w-10 items-center justify-center rounded-lg bg-positive-tint">
              <ArrowRightLeft className="h-5 w-5 text-positive" />
            </div>
            <div>
              <p className="font-heading text-caption font-bold uppercase tracking-wider text-fg-muted">
                {t("playerProfile.loanStatus")}
              </p>
              <p className="text-body font-semibold text-fg">{relationship}</p>
            </div>
          </div>
          <div className="grid gap-2 text-caption text-fg-muted sm:grid-cols-3 sm:text-right">
            <span className="inline-flex items-center gap-1 sm:justify-end">
              <CalendarDays className="h-3.5 w-3.5" />
              {t("playerProfile.loanUntil", {
                date: formatDate(loan.end_date, language),
              })}
            </span>
            <span className="inline-flex items-center gap-1 sm:justify-end">
              <Percent className="h-3.5 w-3.5" />
              {t("playerProfile.loanWageSplit", {
                percent: loan.wage_contribution_pct,
              })}
            </span>
            {loan.buy_option_fee ? (
              <span className="inline-flex items-center gap-1 sm:justify-end">
                <BadgeEuro className="h-3.5 w-3.5" />
                {t("playerProfile.loanBuyOption", {
                  fee: formatExactMoney(loan.buy_option_fee),
                })}
              </span>
            ) : null}
          </div>
        </div>
        {recallable && onGameUpdate ? (
          <button type="button" disabled={busy} onClick={handleRecall}>
            {t(busy ? "transfers.recallingLoan" : "transfers.recallLoan")}
          </button>
        ) : null}
        {error ? <p role="alert">{error}</p> : null}
      </CardBody>
    </Card>
  );
}
