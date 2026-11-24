import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  decideScholarshipAction,
  getYouthAcademyOverview,
  recallYouthLoanBreach,
} from "../../services/youthAcademyService";
import type { GameStateData } from "../../store/gameStore";
import type { YouthAcademyOverview } from "../../store/generated/YouthAcademyOverview";
import { resolveBackendError } from "../../utils/backendI18n";
import { Badge, Button, Card, CardBody, CardHeader } from "../ui";

export default function YouthAcademyOverviewPanel({
  refreshKey,
  onGameUpdate,
}: {
  refreshKey: string;
  onGameUpdate?: (game: GameStateData) => void;
}) {
  const { t } = useTranslation();
  const [result, setResult] = useState<{ key: string; data: YouthAcademyOverview | null } | null>(
    null,
  );
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [revision, setRevision] = useState(0);
  const generation = useRef(0);
  useEffect(() => {
    const current = ++generation.current;
    setError(null);
    void getYouthAcademyOverview()
      .then((data) => {
        if (generation.current === current) setResult({ key: refreshKey, data });
      })
      .catch((err) => {
        if (generation.current === current) setError(resolveBackendError(err));
      });
    return () => {
      generation.current++;
    };
  }, [refreshKey, revision]);
  const act = async (request: () => Promise<GameStateData>) => {
    if (busy) return;
    const current = generation.current;
    setBusy(true);
    setError(null);
    try {
      const updated = await request();
      if (generation.current === current) {
        onGameUpdate?.(updated);
        setRevision((value) => value + 1);
      }
    } catch (err) {
      if (generation.current === current) setError(resolveBackendError(err));
    } finally {
      setBusy(false);
    }
  };
  const data = result?.key === refreshKey ? result.data : null;
  if (!data && !error) return null;
  return (
    <Card>
      <CardHeader>
        {t("youthAcademy.overviewTitle")}{" "}
        {data && <Badge>{t(`youthAcademy.tier${data.academy_tier}`)}</Badge>}
      </CardHeader>
      <CardBody className="flex flex-col gap-4">
        {error && (
          <div role="alert" className="text-danger">
            {error}
          </div>
        )}
        {data && (
          <>
            <section>
              <h4 className="font-bold">{t("youthAcademy.homeGrown")}</h4>
              {(["club", "nation"] as const).map((kind) => (
                <div key={kind} className="flex flex-wrap items-center gap-2">
                  <span>
                    {t(`youthAcademy.${kind}Trained`)}: {data.home_grown[`${kind}_trained_count`]} /{" "}
                    {data.home_grown[`${kind}_trained_quota`]}
                  </span>
                  {data.home_grown[`${kind}_trained_shortfall`] > 0 && (
                    <Badge variant="danger">
                      {t("youthAcademy.shortfall", {
                        count: data.home_grown[`${kind}_trained_shortfall`],
                      })}
                    </Badge>
                  )}
                </div>
              ))}
            </section>
            <section>
              <h4 className="font-bold">{t("youthAcademy.contracts")}</h4>
              {data.scholars.length === 0 && <p>{t("youthAcademy.noContracts")}</p>}
              {data.scholars.map((player) => (
                <div
                  key={player.player_id}
                  className="flex flex-wrap items-center gap-2 border-b border-line py-2"
                >
                  <span>
                    {player.full_name}, {player.age} — {player.scholarship_deadline}
                  </span>
                  {player.decision_due && (
                    <Badge variant="danger">{t("youthAcademy.decisionDue")}</Badge>
                  )}
                  <Button
                    size="sm"
                    disabled={busy || !onGameUpdate}
                    onClick={() => void act(() => decideScholarshipAction(player.player_id, true))}
                  >
                    {t("youthAcademy.offerPro")}
                  </Button>
                  <Button
                    size="sm"
                    variant="outline"
                    disabled={busy || !onGameUpdate}
                    onClick={() => void act(() => decideScholarshipAction(player.player_id, false))}
                  >
                    {t("youthAcademy.releaseScholar")}
                  </Button>
                </div>
              ))}
            </section>
            <section>
              <h4 className="font-bold">{t("youthAcademy.loanedYouth")}</h4>
              {data.loaned_youth.length === 0 && <p>{t("youthAcademy.noLoans")}</p>}
              {data.loaned_youth.map((player) => (
                <div
                  key={player.player_id}
                  className="flex flex-wrap items-center gap-2 border-b border-line py-2"
                >
                  <span>
                    {player.full_name} — {player.loan_team_name}
                  </span>
                  {player.guaranteed_minutes_pct !== null && (
                    <span>
                      {t("youthAcademy.guaranteedMinutes")}: {player.actual_minutes} /{" "}
                      {player.required_minutes ?? "–"} ({player.guaranteed_minutes_pct}%)
                      {player.available_minutes !== null && ` / ${player.available_minutes}`}
                    </span>
                  )}
                  {player.is_breach && (
                    <Badge variant="danger">{t("youthAcademy.minutesBreach")}</Badge>
                  )}
                  {player.is_breach && player.loan_recallable && (
                    <Button
                      size="sm"
                      disabled={busy || !onGameUpdate}
                      onClick={() => void act(() => recallYouthLoanBreach(player.player_id))}
                    >
                      {t("youthAcademy.recallBreach")}
                    </Button>
                  )}
                </div>
              ))}
            </section>
          </>
        )}
      </CardBody>
    </Card>
  );
}
