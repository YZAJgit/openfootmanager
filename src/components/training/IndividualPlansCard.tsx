import { useEffect, useId, useMemo, useState } from "react";
import { Pencil, Plus, Trash2, UserCheck } from "lucide-react";
import { useTranslation } from "react-i18next";

import type { GameStateData, PlayerData, TeamData } from "../../store/gameStore";
import {
  clearIndividualDevelopmentPlan,
  getIndividualPlanOptions,
  setIndividualDevelopmentPlan,
  type IndividualPlanOptions,
  type TrainingGroupData,
} from "../../services/trainingService";
import { resolveBackendError } from "../../utils/backendI18n";
import { Badge, Button, Card, CardBody, CardHeader, Select } from "../ui";

const INDIVIDUAL_PLAN_PREFIX = "individual:";

interface IndividualPlansCardProps {
  team: TeamData | null;
  onGameUpdate?: (state: GameStateData) => void;
  roster: PlayerData[];
  allSquadPlayers?: PlayerData[];
  isSaving: boolean;
  setIsSaving: (value: boolean) => void;
  trainingFocusIds: readonly string[];
  intensityIds: readonly string[];
  optionsRevision?: unknown;
}

function isIndividualPlanGroup(group: TrainingGroupData): boolean {
  return (
    group.id.startsWith(INDIVIDUAL_PLAN_PREFIX) &&
    group.player_ids.length === 1 &&
    group.intensity != null
  );
}

function normalizeOptions(value: unknown): IndividualPlanOptions | null {
  if (!value || typeof value !== "object") return null;
  const candidate = value as Partial<IndividualPlanOptions>;
  if (
    typeof candidate.max_plans !== "number" ||
    !Array.isArray(candidate.eligible_mentors) ||
    !Array.isArray(candidate.eligible_mentees)
  ) {
    return null;
  }
  return {
    max_plans: candidate.max_plans,
    eligible_mentors: candidate.eligible_mentors,
    eligible_mentees: candidate.eligible_mentees,
  };
}

interface ActivePlanRowProps {
  plan: TrainingGroupData;
  playerName: string;
  mentorName: string | null;
  isSaving: boolean;
  onEdit: (plan: TrainingGroupData) => void;
  onClear: (playerId: string) => void;
}

function ActivePlanRow({
  plan,
  playerName,
  mentorName,
  isSaving,
  onEdit,
  onClear,
}: ActivePlanRowProps) {
  const { t } = useTranslation();
  const playerId = plan.player_ids[0] ?? "";
  const intensity = plan.intensity ?? "Medium";

  return (
    <li className="flex flex-wrap items-center justify-between gap-3 rounded-lg border border-line bg-sunken px-3 py-2.5">
      <div className="min-w-0 flex-1">
        <div className="flex flex-wrap items-center gap-2">
          <span className="truncate text-body font-semibold text-fg">{playerName}</span>
          <Badge variant="neutral" size="sm">
            {t(`training.focuses.${plan.focus}.label`)}
          </Badge>
          <Badge variant="outline" size="sm">
            {t(`training.intensities.${intensity}.label`)}
          </Badge>
        </div>
        {mentorName ? (
          <p className="mt-1 flex items-center gap-1.5 text-caption text-fg-muted">
            <UserCheck className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
            <span>{t("training.individualPlans.mentorTag", { mentor: mentorName })}</span>
          </p>
        ) : null}
      </div>
      <div className="flex items-center gap-1.5">
        <Button
          type="button"
          variant="ghost"
          size="sm"
          disabled={isSaving}
          onClick={() => onEdit(plan)}
          icon={<Pencil />}
          aria-label={`${t("training.individualPlans.editPlan")} ${playerName}`}
        >
          {t("training.individualPlans.editPlan")}
        </Button>
        <Button
          type="button"
          variant="ghost"
          size="sm"
          disabled={isSaving}
          onClick={() => onClear(playerId)}
          icon={<Trash2 />}
          className="text-danger hover:text-danger"
          aria-label={`${t("training.individualPlans.clearPlan")} ${playerName}`}
        >
          {t("training.individualPlans.clearPlan")}
        </Button>
      </div>
    </li>
  );
}

export default function IndividualPlansCard({
  team,
  onGameUpdate,
  roster,
  allSquadPlayers,
  isSaving,
  setIsSaving,
  trainingFocusIds,
  intensityIds,
  optionsRevision,
}: IndividualPlansCardProps) {
  const { t } = useTranslation();
  const playerLabelId = useId();
  const focusLabelId = useId();
  const intensityLabelId = useId();
  const mentorLabelId = useId();

  const [options, setOptions] = useState<IndividualPlanOptions | null>(null);
  const [optionsLoading, setOptionsLoading] = useState(true);
  const [optionsError, setOptionsError] = useState<string | null>(null);
  const [selectedPlayerId, setSelectedPlayerId] = useState("");
  const [selectedFocus, setSelectedFocus] = useState<string>(trainingFocusIds[0] ?? "Physical");
  const [selectedIntensity, setSelectedIntensity] = useState<string>(intensityIds[1] ?? "Medium");
  const [selectedMentorId, setSelectedMentorId] = useState("");
  const [errorMessage, setErrorMessage] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setOptionsLoading(true);
    setOptionsError(null);
    getIndividualPlanOptions()
      .then((result) => {
        if (cancelled) return;
        const normalized = normalizeOptions(result);
        setOptions(normalized);
        if (normalized === null) {
          setOptionsError(resolveBackendError("Failed to load individual plan options"));
        }
      })
      .catch((error: unknown) => {
        console.error("Failed to load individual plan options:", error);
        if (!cancelled) {
          setOptions(null);
          setOptionsError(resolveBackendError(error) || "Failed to load individual plan options");
        }
      })
      .finally(() => {
        if (!cancelled) {
          setOptionsLoading(false);
        }
      });
    return () => {
      cancelled = true;
    };
  }, [team?.id, optionsRevision]);

  const plans = useMemo(
    () => (team?.training_groups ?? []).filter(isIndividualPlanGroup),
    [team?.training_groups],
  );

  const sortedRoster = useMemo(
    () => [...roster].sort((left, right) => left.match_name.localeCompare(right.match_name)),
    [roster],
  );

  const playerNameById = useMemo(() => {
    const map = new Map<string, string>();
    for (const player of allSquadPlayers ?? roster) {
      map.set(player.id, player.match_name);
    }
    for (const mentor of options?.eligible_mentors ?? []) {
      if (!map.has(mentor.id)) map.set(mentor.id, mentor.match_name);
    }
    for (const mentee of options?.eligible_mentees ?? []) {
      if (!map.has(mentee.id)) map.set(mentee.id, mentee.match_name);
    }
    return map;
  }, [allSquadPlayers, roster, options]);

  const eligibleMenteeIds = useMemo(
    () => new Set((options?.eligible_mentees ?? []).map((mentee) => mentee.id)),
    [options],
  );

  const availableMentors = useMemo(
    () => (options?.eligible_mentors ?? []).filter((mentor) => mentor.id !== selectedPlayerId),
    [options, selectedPlayerId],
  );

  const optionsReady = options !== null && !optionsLoading && optionsError === null;
  const maxPlans = options?.max_plans ?? null;
  const existingPlanForSelected = plans.find((plan) => plan.player_ids[0] === selectedPlayerId);
  const isEditingExisting = existingPlanForSelected !== undefined;
  const isAtLimit = maxPlans !== null && plans.length >= maxPlans;
  const canUseMentor =
    optionsReady &&
    selectedPlayerId !== "" &&
    eligibleMenteeIds.has(selectedPlayerId) &&
    availableMentors.length > 0;
  const effectiveMentorId = canUseMentor ? selectedMentorId : "";

  const handleSelectPlayer = (playerId: string) => {
    setSelectedPlayerId(playerId);
    setErrorMessage(null);
    const existing = plans.find((plan) => plan.player_ids[0] === playerId);
    if (existing) {
      setSelectedFocus(existing.focus);
      setSelectedIntensity(existing.intensity ?? "Medium");
      setSelectedMentorId(existing.mentor_id ?? "");
    } else {
      setSelectedMentorId("");
    }
  };

  const handleEditPlan = (plan: TrainingGroupData) => {
    const playerId = plan.player_ids[0] ?? "";
    setSelectedPlayerId(playerId);
    setSelectedFocus(plan.focus);
    setSelectedIntensity(plan.intensity ?? "Medium");
    setSelectedMentorId(plan.mentor_id ?? "");
    setErrorMessage(null);
  };

  const handleSavePlan = async () => {
    if (!selectedPlayerId || !optionsReady) return;
    setIsSaving(true);
    setErrorMessage(null);
    try {
      const updated = await setIndividualDevelopmentPlan(
        selectedPlayerId,
        selectedFocus,
        selectedIntensity,
        effectiveMentorId || null,
      );
      onGameUpdate?.(updated);
    } catch (error: unknown) {
      setErrorMessage(resolveBackendError(error));
    } finally {
      setIsSaving(false);
    }
  };

  const handleClearPlan = async (playerId: string) => {
    setIsSaving(true);
    setErrorMessage(null);
    try {
      const updated = await clearIndividualDevelopmentPlan(playerId);
      onGameUpdate?.(updated);
      if (selectedPlayerId === playerId) {
        setSelectedMentorId("");
      }
    } catch (error: unknown) {
      setErrorMessage(resolveBackendError(error));
    } finally {
      setIsSaving(false);
    }
  };

  const displayedError = optionsError ?? errorMessage;
  const mentorHint =
    !optionsReady || selectedPlayerId === ""
      ? null
      : !eligibleMenteeIds.has(selectedPlayerId)
        ? t("training.individualPlans.menteeAgeHint")
        : availableMentors.length === 0
          ? t("training.individualPlans.noEligibleMentors")
          : null;

  return (
    <Card>
      <CardHeader
        action={
          maxPlans !== null ? (
            <Badge variant={isAtLimit ? "accent" : "neutral"} size="sm">
              {t("training.individualPlans.slotCount", { count: plans.length, max: maxPlans })}
            </Badge>
          ) : null
        }
      >
        {t("training.individualPlans.title")}
      </CardHeader>
      <CardBody>
        {maxPlans !== null ? (
          <p className="mb-3 text-caption text-fg-muted">
            {t("training.individualPlans.description", { max: maxPlans })}
          </p>
        ) : null}

        {displayedError ? (
          <div
            role="alert"
            className="mb-3 rounded-lg border border-danger bg-danger-tint px-3 py-2 text-caption text-danger"
          >
            {displayedError}
          </div>
        ) : null}

        {plans.length === 0 ? (
          <p className="mb-4 text-body text-fg-muted">{t("training.individualPlans.empty")}</p>
        ) : (
          <ul className="mb-4 flex flex-col gap-2">
            {plans.map((plan) => {
              const playerId = plan.player_ids[0] ?? "";
              const playerName = playerNameById.get(playerId) ?? plan.name;
              const mentorName = plan.mentor_id
                ? (playerNameById.get(plan.mentor_id) ?? plan.mentor_id)
                : null;
              return (
                <ActivePlanRow
                  key={plan.id}
                  plan={plan}
                  playerName={playerName}
                  mentorName={mentorName}
                  isSaving={isSaving}
                  onEdit={handleEditPlan}
                  onClear={handleClearPlan}
                />
              );
            })}
          </ul>
        )}

        <div className="rounded-lg border border-line bg-sunken p-3">
          <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
            <div>
              <span
                id={playerLabelId}
                className="mb-1 block text-caption font-semibold text-fg-muted"
              >
                {t("common.player")}
              </span>
              <Select
                value={selectedPlayerId}
                onChange={(event) => handleSelectPlayer(event.target.value)}
                disabled={isSaving || sortedRoster.length === 0}
                selectSize="sm"
                fullWidth
                aria-labelledby={playerLabelId}
              >
                <option value="">{t("training.individualPlans.selectPlayer")}</option>
                {sortedRoster.map((player) => (
                  <option key={player.id} value={player.id}>
                    {player.match_name}
                  </option>
                ))}
              </Select>
            </div>

            <div>
              <span
                id={focusLabelId}
                className="mb-1 block text-caption font-semibold text-fg-muted"
              >
                {t("training.effectiveFocus")}
              </span>
              <Select
                value={selectedFocus}
                onChange={(event) => setSelectedFocus(event.target.value)}
                disabled={isSaving}
                selectSize="sm"
                fullWidth
                aria-labelledby={focusLabelId}
              >
                {trainingFocusIds.map((focusId) => (
                  <option key={focusId} value={focusId}>
                    {t(`training.focuses.${focusId}.label`)}
                  </option>
                ))}
              </Select>
            </div>

            <div>
              <span
                id={intensityLabelId}
                className="mb-1 block text-caption font-semibold text-fg-muted"
              >
                {t("training.intensity")}
              </span>
              <Select
                value={selectedIntensity}
                onChange={(event) => setSelectedIntensity(event.target.value)}
                disabled={isSaving}
                selectSize="sm"
                fullWidth
                aria-labelledby={intensityLabelId}
              >
                {intensityIds.map((intensityId) => (
                  <option key={intensityId} value={intensityId}>
                    {t(`training.intensities.${intensityId}.label`)}
                  </option>
                ))}
              </Select>
            </div>

            <div>
              <span
                id={mentorLabelId}
                className="mb-1 block text-caption font-semibold text-fg-muted"
              >
                {t("training.individualPlans.mentorLabel")}
              </span>
              <Select
                value={effectiveMentorId}
                onChange={(event) => setSelectedMentorId(event.target.value)}
                disabled={isSaving || !canUseMentor}
                selectSize="sm"
                fullWidth
                aria-labelledby={mentorLabelId}
              >
                <option value="">{t("training.individualPlans.noMentor")}</option>
                {availableMentors.map((mentor) => (
                  <option key={mentor.id} value={mentor.id}>
                    {t("training.individualPlans.mentorOption", {
                      name: mentor.match_name,
                      age: mentor.age,
                    })}
                  </option>
                ))}
              </Select>
              {mentorHint ? <p className="mt-1 text-caption text-fg-subtle">{mentorHint}</p> : null}
            </div>
          </div>

          <div className="mt-3 flex flex-wrap items-center justify-between gap-2">
            <div>
              {isAtLimit && !isEditingExisting && maxPlans !== null ? (
                <p className="text-caption text-fg-muted">
                  {t("training.individualPlans.limitReached", { max: maxPlans })}
                </p>
              ) : null}
            </div>
            <Button
              type="button"
              variant="primary"
              size="sm"
              disabled={
                isSaving || !optionsReady || !selectedPlayerId || (isAtLimit && !isEditingExisting)
              }
              onClick={handleSavePlan}
              icon={<Plus />}
            >
              {isEditingExisting
                ? t("training.individualPlans.updatePlan")
                : t("training.individualPlans.addPlan")}
            </Button>
          </div>
        </div>
      </CardBody>
    </Card>
  );
}
