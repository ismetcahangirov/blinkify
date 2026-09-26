import type { ExportReport, ReportSegment } from "@blinkify/types";
import { Button, Dialog, DialogClose, Switch } from "@blinkify/ui";
import { invoke } from "@tauri-apps/api/core";
import { save as saveDialog } from "@tauri-apps/plugin-dialog";
import { useEffect, useState } from "react";

import { size } from "./exportJobs.js";
import { useExportJobs } from "./exportJobs.store.js";

/**
 * The export report (#52): what an export did, measured on the file it
 * wrote — the engine's report, shown. Every line is the engine's; the
 * dialog lays it out, and offers it as plain text for a bug report, without
 * folders unless the user includes them.
 */
export function ExportReportDialog() {
  const id = useExportJobs((state) => state.reportOf);
  const close = useExportJobs((state) => state.closeReport);
  if (id === null) return null;
  return (
    <ReportBody
      key={id}
      id={id}
      onOpenChange={(open) => {
        if (!open) close();
      }}
    />
  );
}

/** `1:05.4` for a time on the sequence. */
export function clock(seconds: number): string {
  const minutes = Math.floor(seconds / 60);
  return `${minutes}:${(seconds - minutes * 60).toFixed(1).padStart(4, "0")}`;
}

/** What happened to a segment, in words. */
export function executionWords(segment: ReportSegment): string {
  switch (segment.execution) {
    case "copied":
      return "copied";
    case "partly-re-encoded":
      return "smart-cut: partly re-encoded";
    case "re-encoded":
      return "re-encoded";
    case "empty":
      return "no packets";
  }
}

function ReportBody({
  id,
  onOpenChange,
}: {
  readonly id: number;
  readonly onOpenChange: (open: boolean) => void;
}) {
  const [report, setReport] = useState<ExportReport | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [withPaths, setWithPaths] = useState(false);
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    let stale = false;
    invoke<ExportReport>("export_report", { id })
      .then((answer) => {
        if (!stale) setReport(answer);
      })
      .catch((cause: unknown) => {
        if (!stale) setProblem(String(cause));
      });
    return () => {
      stale = true;
    };
  }, [id]);

  const copy = async () => {
    try {
      const text = await invoke<string>("export_report_text", {
        id,
        withPaths,
      });
      await navigator.clipboard.writeText(text);
      setCopied(true);
    } catch (cause) {
      setProblem(String(cause));
    }
  };

  const saveAs = async () => {
    const path = await saveDialog({
      defaultPath: `${report?.name ?? "export"} report.txt`,
      filters: [{ name: "Text", extensions: ["txt"] }],
    });
    if (!path) return;
    try {
      await invoke("save_export_report", { id, path, withPaths });
    } catch (cause) {
      setProblem(String(cause));
    }
  };

  return (
    <Dialog
      open
      onOpenChange={onOpenChange}
      title="Export report"
      description="What the export did, measured on the file it wrote: each packet compared with its source's."
      className="export-report"
      actions={
        <>
          <Switch
            label="Include folders"
            checked={withPaths}
            onCheckedChange={(value) => {
              setWithPaths(value);
              setCopied(false);
            }}
          />
          <Button variant="secondary" onClick={() => void saveAs()}>
            Save as text…
          </Button>
          <Button variant="secondary" onClick={() => void copy()}>
            {copied ? "Copied" : "Copy as text"}
          </Button>
          <DialogClose>
            <Button variant="primary">Close</Button>
          </DialogClose>
        </>
      }
    >
      {report === null ? (
        <p className="export-dialog__note" role={problem ? "alert" : undefined}>
          {problem ?? "Reading the report…"}
        </p>
      ) : (
        <ReportView report={report} />
      )}
      {report !== null && problem ? (
        <p className="export-dialog__problem" role="alert">
          {problem}
        </p>
      ) : null}
    </Dialog>
  );
}

function ReportView({ report }: { readonly report: ExportReport }) {
  return (
    <div className="export-report__body" data-testid="export-report">
      <p
        className="export-dialog__badge"
        data-state={report.lossless ? "lossless" : "re-encoded"}
      >
        {report.summary}
      </p>
      <dl className="export-dialog__facts">
        <dt>Output</dt>
        <dd>
          {report.output.name} — {report.output.container},{" "}
          {report.output.codecs.join(" + ")}, {size(report.output.sizeBytes)}
        </dd>
        {report.sources.map((source) => (
          <FactRow key={source.path} label="Source">
            {source.name} — {source.container}, {source.codecs.join(" + ")},{" "}
            {size(source.sizeBytes)}
          </FactRow>
        ))}
        {report.video ? (
          <FactRow label="Pictures">
            {report.video.copiedSeconds.toFixed(2)} s copied,{" "}
            {report.video.reEncodedSeconds.toFixed(2)} s re-encoded;{" "}
            {report.video.identical} of {report.video.packets} packets
            bit-identical
          </FactRow>
        ) : null}
        {report.audio ? (
          <FactRow label="Sound">
            {report.audio.copiedSeconds.toFixed(2)} s copied,{" "}
            {report.audio.reEncodedSeconds.toFixed(2)} s re-encoded;{" "}
            {report.audio.identical} of {report.audio.packets} packets
            bit-identical
          </FactRow>
        ) : null}
        <dt>Bit-identical</dt>
        <dd>
          {report.identicalPercent.toFixed(1)} % of the output&apos;s packets
        </dd>
      </dl>
      <ol className="export-report__segments">
        {report.segments.map((segment, index) => (
          <li
            key={index}
            data-execution={segment.execution}
            data-as-planned={segment.asPlanned ? "true" : "false"}
          >
            <span className="export-dialog__timecode">
              {clock(segment.startSeconds)}–{clock(segment.endSeconds)}
            </span>{" "}
            {segment.media === "video" ? "Pictures" : "Sound"}:{" "}
            {executionWords(segment)} ({segment.identical} of {segment.packets}{" "}
            packets identical)
            {segment.encoder ? <div>Encoded with {segment.encoder}</div> : null}
            {segment.reasons.map((reason) => (
              <div key={reason}>Why: {reason}</div>
            ))}
            {segment.suggestions.map((suggestion) => (
              <div key={suggestion} className="export-report__suggestion">
                Instead: {suggestion}
              </div>
            ))}
          </li>
        ))}
      </ol>
    </div>
  );
}

function FactRow({
  label,
  children,
}: {
  readonly label: string;
  readonly children: React.ReactNode;
}) {
  return (
    <>
      <dt>{label}</dt>
      <dd>{children}</dd>
    </>
  );
}
