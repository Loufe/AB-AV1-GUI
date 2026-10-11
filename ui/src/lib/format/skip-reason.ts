import type { SkipReason } from "@/lib/bindings";

export function skipReasonText(reason: SkipReason): { reason: string; detail: string | null } {
  if (reason === "AlreadyAv1Matroska") {
    return { reason: "already AV1", detail: "Already AV1 in an MKV container — nothing to do" };
  }
  if (reason === "OutputExists") {
    return { reason: "output exists", detail: "The output file already exists" };
  }
  if (reason === "AlreadyQueued") {
    return { reason: "already queued", detail: "This path is already in the queue" };
  }
  if (reason === "PathNotPersistable") {
    return {
      reason: "unsupported path",
      detail: "This path contains characters the queue cannot store",
    };
  }
  if ("LowResolution" in reason && reason.LowResolution !== undefined) {
    return {
      reason: "below minimum resolution",
      detail: `${reason.LowResolution.pixels.toLocaleString()} px is under the ${reason.LowResolution.minimum.toLocaleString()} px minimum`,
    };
  }
  if ("AlreadyConverted" in reason && reason.AlreadyConverted !== undefined) {
    return {
      reason: "already converted",
      detail: "This file is the output of a previous conversion",
    };
  }
  if ("NotWorthwhile" in reason && reason.NotWorthwhile !== undefined) {
    return {
      reason: "not worthwhile",
      detail: "A previous analysis found no worthwhile savings",
    };
  }
  return {
    reason: "duplicate content",
    detail: "Identical content was already converted",
  };
}
