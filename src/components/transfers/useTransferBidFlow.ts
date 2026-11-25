import { useEffect, useRef, useState } from "react";

import type { GameStateData, PlayerData, TeamData, TransferOfferData } from "../../store/gameStore";
import {
  type BidNegotiationStatusData,
  getBidNegotiationStatus,
  makeTransferBid,
  makeStructuredTransferBid,
  previewTransferBidFinancialImpact,
  type TransferBidProjectionData,
  type TransferNegotiationFeedbackData,
  type TransferNegotiationResponseData,
} from "../../services/transfersService";
import { getErrorMessage, resolveTranslatedErrorMessage } from "../../utils/errorMessage";
import {
  buildResumedBidFeedback,
  normalizeTransferNegotiationFeedback,
} from "./TransfersTab.helpers";

interface UseTransferBidFlowArgs {
  gameState: GameStateData;
  onGameUpdate?: (game: GameStateData) => void;
}

export interface UseTransferBidFlowResult {
  installments: {
    enabled: boolean;
    upfront: number;
    count: number;
    interval: number;
    setEnabled: (v: boolean) => void;
    setUpfront: (v: number) => void;
    setCount: (v: number) => void;
    setInterval: (v: number) => void;
  };
  bidTarget: PlayerData | null;
  bidAmount: string;
  setBidAmount: (value: string) => void;
  bidResult: TransferNegotiationResponseData["decision"] | "error" | null;
  /** The translated reason a bid failed. Set only when `bidResult` is `"error"`. */
  bidError: string | null;
  bidLoading: boolean;
  bidFeedback: TransferNegotiationFeedbackData | null;
  bidProjection: TransferBidProjectionData["projection"] | null;
  bidFee: number | null;
  activeBidOffer: TransferOfferData | null;
  /** The day broken-off talks can resume; set means the bid form is replaced by that date. */
  talksResumeOn: string | null;
  myTeam: TeamData | null;
  hasExistingOffer: boolean;
  bidSubmitDisabled: boolean;
  openBidNegotiation: (player: PlayerData) => void;
  closeBidNegotiation: () => void;
  handleMakeBid: () => Promise<void>;
}

export function useTransferBidFlow({
  gameState,
  onGameUpdate,
}: UseTransferBidFlowArgs): UseTransferBidFlowResult {
  const myTeam = gameState.teams.find((team) => team.id === gameState.manager.team_id) ?? null;
  // The id, not the player: the player is read from the game state on every render, so the panel
  // sees the round and the offers a bid has just advanced instead of the ones it opened with.
  const [bidTargetId, setBidTargetId] = useState<string | null>(null);
  const bidTarget =
    bidTargetId === null
      ? null
      : (gameState.players.find((player) => player.id === bidTargetId) ?? null);
  const [bidAmount, setBidAmount] = useState("");
  const [enabled, setEnabledState] = useState(false);
  const [upfront, setUpfront] = useState(0);
  const [count, setCount] = useState(2);
  const [interval, setInterval] = useState(90);
  const [bidResult, setBidResult] = useState<
    TransferNegotiationResponseData["decision"] | "error" | null
  >(null);
  const [bidLoading, setBidLoading] = useState(false);
  const [bidFeedback, setBidFeedback] = useState<TransferNegotiationFeedbackData | null>(null);
  // Separate from `bidResult` on purpose. `bidResult` is the backend's *decision* and the UI
  // switches on it; the message explaining a failure is a different thing and needs
  // translating. Before this they shared one variable, which only type-checked because the
  // catch clause was `any`.
  const [bidError, setBidError] = useState<string | null>(null);
  const [bidProjection, setBidProjection] = useState<
    TransferBidProjectionData["projection"] | null
  >(null);

  // What the backend says the next bid continues (or that talks are broken off until a day): the
  // cooldown is its rule, so the panel asks rather than computing it from the offers.
  const [negotiation, setNegotiation] = useState<BidNegotiationStatusData | null>(null);
  // Opening clears what the panel knew, so every opening, even of the player already open, asks again.
  const [openings, setOpenings] = useState(0);
  useEffect(() => {
    if (bidTargetId === null) {
      setNegotiation(null);
      return;
    }
    let cancelled = false;
    getBidNegotiationStatus(bidTargetId)
      .then((status) => {
        if (!cancelled) setNegotiation(status);
      })
      .catch(() => {
        if (!cancelled) setNegotiation(null);
      });
    return () => {
      cancelled = true;
    };
  }, [bidTargetId, gameState, openings]);

  const activeBidOffer =
    bidTarget && negotiation?.offer_id
      ? (bidTarget.transfer_offers.find((offer) => offer.id === negotiation.offer_id) ?? null)
      : null;
  const talksResumeOn = negotiation?.talks_resume_on ?? null;

  // Resuming a negotiation pre-fills the amount and the feedback once per opening, when the
  // backend's answer first names one; a later answer (after a bid) must not overwrite them.
  const resumedFor = useRef<string | null>(null);
  useEffect(() => {
    // Broken-off talks resume nothing: the panel shows the day they can instead of the form.
    if (!bidTarget || !activeBidOffer || talksResumeOn !== null) return;
    if (resumedFor.current === bidTarget.id) return;
    resumedFor.current = bidTarget.id;
    setBidAmount(
      ((activeBidOffer.suggested_counter_fee ?? activeBidOffer.fee) / 1_000_000).toFixed(2),
    );
    setBidFeedback(buildResumedBidFeedback(activeBidOffer));
  }, [bidTarget, activeBidOffer, talksResumeOn]);
  const bidAmountMillions = Number.parseFloat(bidAmount);
  const bidFee = Number.isFinite(bidAmountMillions)
    ? Math.round(bidAmountMillions * 1_000_000)
    : null;

  const setEnabled = (value: boolean): void => {
    setEnabledState(value);
    if (value) {
      setUpfront(Math.floor((bidFee ?? 0) / 2));
      setCount(2);
      setInterval(90);
    }
  };

  useEffect(() => {
    if (!bidTarget || bidFee === null || bidFee <= 0) {
      setBidProjection(null);
      return;
    }

    let cancelled = false;

    const loadProjection = async (): Promise<void> => {
      try {
        const result = await (enabled
          ? previewTransferBidFinancialImpact(bidTarget.id, bidFee, upfront)
          : previewTransferBidFinancialImpact(bidTarget.id, bidFee));

        if (!cancelled) {
          setBidProjection(result.projection ?? null);
        }
      } catch {
        if (!cancelled) {
          setBidProjection(null);
        }
      }
    };

    void loadProjection();

    return () => {
      cancelled = true;
    };
  }, [bidFee, bidTarget, enabled, upfront]);

  const bidSessionRef = useRef(0);
  useEffect(
    () => () => {
      bidSessionRef.current += 1;
    },
    [],
  );

  const openBidNegotiation = (player: PlayerData): void => {
    bidSessionRef.current += 1;
    setBidLoading(false);
    setEnabledState(false);
    setUpfront(0);
    setCount(2);
    setInterval(90);
    resumedFor.current = null;
    setOpenings((count) => count + 1);
    setBidTargetId(player.id);
    setBidAmount((player.market_value / 1_000_000).toFixed(1));
    setBidResult(null);
    setBidError(null);
    setBidFeedback(null);
    setBidProjection(null);
    setNegotiation(null);
  };

  const closeBidNegotiation = (): void => {
    bidSessionRef.current += 1;
    setBidLoading(false);
    setEnabledState(false);
    setUpfront(0);
    setCount(2);
    setInterval(90);
    resumedFor.current = null;
    setBidTargetId(null);
    setBidAmount("");
    setBidResult(null);
    setBidError(null);
    setBidFeedback(null);
    setBidProjection(null);
  };

  const handleMakeBid = async (): Promise<void> => {
    if (!bidTarget || bidFee === null || bidFee <= 0) {
      return;
    }

    const session = bidSessionRef.current;
    setBidLoading(true);
    setBidResult(null);
    setBidError(null);
    setBidFeedback(null);

    try {
      const response = await (enabled
        ? makeStructuredTransferBid(bidTarget.id, bidFee, upfront, count, interval)
        : makeTransferBid(bidTarget.id, bidFee));
      if (bidSessionRef.current !== session) return;
      // The player is handled now: an offer the game update brings is this bid's own, and its
      // feedback is the response's, not a "resumed negotiation" one.
      resumedFor.current = bidTarget.id;
      setBidResult(response.decision);
      setBidError(null);
      setBidFeedback(normalizeTransferNegotiationFeedback(response.feedback));
      onGameUpdate?.(response.game);

      // `suggested_fee` is in raw euros, but this input is denominated in
      // millions (see `bidFee` above and `openBidNegotiation`). Convert
      // before pre-filling, or the next submit sends 1,000,000× the value.
      if (response.suggested_fee !== null) {
        setBidAmount((response.suggested_fee / 1_000_000).toFixed(2));
      }
    } catch (error: unknown) {
      if (bidSessionRef.current !== session) return;
      setBidResult("error");
      setBidError(resolveTranslatedErrorMessage(getErrorMessage(error)));
      setBidFeedback(null);
    } finally {
      if (bidSessionRef.current === session) setBidLoading(false);
    }
  };

  return {
    installments: {
      enabled,
      upfront,
      count,
      interval,
      setEnabled,
      setUpfront,
      setCount,
      setInterval,
    },
    bidTarget,
    bidAmount,
    setBidAmount,
    bidResult,
    bidError,
    bidLoading,
    bidFeedback,
    bidProjection,
    bidFee,
    activeBidOffer,
    talksResumeOn,
    myTeam,
    hasExistingOffer: activeBidOffer?.status === "Pending",
    bidSubmitDisabled:
      bidLoading ||
      bidResult === "accepted" ||
      bidFee === null ||
      bidFee <= 0 ||
      talksResumeOn !== null ||
      bidProjection === null ||
      bidProjection.exceeds_transfer_budget ||
      bidProjection.exceeds_finance ||
      bidProjection.blocked_by_wage_policy,
    openBidNegotiation,
    closeBidNegotiation,
    handleMakeBid,
  };
}
