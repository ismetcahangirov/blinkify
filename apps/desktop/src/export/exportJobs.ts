import type { ExportJob } from "@blinkify/types";

/**
 * How an export job is put into words (#51). Pure, so every wording is
 * tested without a window.
 */

/** Still to happen or happening: queued or running. */
export function isActive(job: ExportJob): boolean {
  return job.state.state === "queued" || job.state.state === "running";
}

/** Left by a crash, waiting for the user to restart or discard it. */
export function isInterrupted(job: ExportJob): boolean {
  return job.state.state === "interrupted";
}

/** Reached an end, and belongs to the history. */
export function isFinished(job: ExportJob): boolean {
  return (
    job.state.state === "completed" ||
    job.state.state === "failed" ||
    job.state.state === "cancelled"
  );
}

/** The last part of a Windows or POSIX path. */
export function baseName(path: string): string {
  return path.split(/[\\/]/).pop() ?? path;
}

/** `about 3 min left`, `about 40 s left`: rounded, because it is an estimate. */
export function remaining(seconds: number): string {
  if (seconds >= 90) return `about ${Math.round(seconds / 60)} min left`;
  if (seconds >= 10) return `about ${Math.round(seconds / 10) * 10} s left`;
  return "a few seconds left";
}

/** `1.2 GB`, `640 MB`, `12 KB`. */
export function size(bytes: number): string {
  const units = ["bytes", "KB", "MB", "GB", "TB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1000 && unit < units.length - 1) {
    value /= 1000;
    unit += 1;
  }
  const digits = unit === 0 || value >= 100 ? 0 : 1;
  return `${value.toFixed(digits)} ${units[unit] ?? "bytes"}`;
}

/** What is happening to the job, in one line. */
export function statusLine(job: ExportJob): string {
  const state = job.state;
  switch (state.state) {
    case "queued":
      return "Waiting for the exports before it";
    case "running":
      if (state.stage === "preparing")
        return "Preparing: measuring loudness and planning";
      return state.remainingSeconds === null
        ? `Exporting — ${Math.floor(state.fraction * 100)}%`
        : `Exporting — ${Math.floor(state.fraction * 100)}%, ${remaining(state.remainingSeconds)}`;
    case "completed":
      return `Exported — ${size(state.bytes)}`;
    case "failed":
      return `Failed: ${state.message}. Nothing was left at the target.`;
    case "cancelled":
      return "Cancelled. Nothing was left at the target.";
    case "interrupted":
      return "Interrupted when Blinkify closed";
  }
}
