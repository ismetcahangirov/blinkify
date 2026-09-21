/**
 * The IPC contract between the React renderer and the Rust engine.
 *
 * `CLAUDE.md` section 2: the renderer never reaches the engine directly. It
 * calls Tauri commands and consumes Tauri events, both typed here.
 *
 * Types under `./generated` are emitted from the Rust definitions by
 * `pnpm types:generate` and are not hand-edited. The Rust side is the source of
 * truth — two hand-maintained definitions across a serialisation boundary drift,
 * and a drifting IPC contract is a bug class worth designing out for the cost of
 * one build step.
 */

export * from "./generated/index.js";
