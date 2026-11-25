import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { formatVal } from "../../lib/helpers";
import { getTransferInstallments } from "../../services/financeService";
import type { TransferInstallmentsSummaryView } from "../../store/generated/TransferInstallmentsSummaryView";
import { Badge, Card, CardBody, CardHeader } from "../ui";

export function TransferInstallmentsCard({ refreshKey }: { refreshKey: string }) {
  const { t } = useTranslation();
  const [result, setResult] = useState<{
    key: string;
    data: TransferInstallmentsSummaryView | null;
  } | null>(null);

  useEffect(() => {
    let cancelled = false;
    void getTransferInstallments().then(
      (data) => {
        if (!cancelled) setResult({ key: refreshKey, data });
      },
      () => {
        if (!cancelled) setResult({ key: refreshKey, data: null });
      },
    );
    return () => {
      cancelled = true;
    };
  }, [refreshKey]);

  const data = result?.key === refreshKey ? result.data : null;
  if (!data) return null;

  return (
    <Card className="lg:col-span-3">
      <CardHeader>{t("finances.installments.title")}</CardHeader>
      <CardBody>
        <div className="grid grid-cols-2 gap-4">
          <div>
            <p className="text-caption text-fg-muted">
              {t("finances.installments.outstandingOwed")}
            </p>
            <p className="font-heading font-bold text-danger">{formatVal(data.outstanding_owed)}</p>
          </div>
          <div>
            <p className="text-caption text-fg-muted">
              {t("finances.installments.outstandingReceivable")}
            </p>
            <p className="font-heading font-bold text-positive">
              {formatVal(data.outstanding_receivable)}
            </p>
          </div>
        </div>
        {data.installments.length === 0 ? (
          <p className="mt-4 text-fg-muted">{t("finances.installments.empty")}</p>
        ) : (
          <ul className="mt-4 divide-y divide-line">
            {data.installments.map((item) => (
              <li key={item.id} className="flex flex-wrap items-center justify-between gap-3 py-3">
                <div>
                  <p className="font-bold text-fg">{item.player_name}</p>
                  <p className="text-caption text-fg-muted">{item.counterparty_team_name}</p>
                </div>
                <div className="flex flex-wrap items-center gap-2">
                  <Badge variant={item.direction === "Payable" ? "danger" : "success"}>
                    {t(
                      item.direction === "Payable"
                        ? "finances.installments.payable"
                        : "finances.installments.receivable",
                    )}
                  </Badge>
                  <span className="font-bold">{formatVal(item.amount)}</span>
                  <span className="text-caption text-fg-muted">
                    {t("finances.installments.dueDate", { date: item.due_date })}
                  </span>
                  <Badge variant={item.settled ? "success" : "neutral"}>
                    {t(
                      item.settled
                        ? "finances.installments.settled"
                        : "finances.installments.upcoming",
                    )}
                  </Badge>
                </div>
              </li>
            ))}
          </ul>
        )}
      </CardBody>
    </Card>
  );
}
