// Bundle Monaco locally (no CDN) with only the languages a chip project uses.
import * as monaco from "monaco-editor/editor/editor.api";
// Editor UI contributions: hover, suggest, find, go-to-error, peek, rename, folding, ...
import "monaco-editor/features/register.all";
import "monaco-editor/languages/definitions/systemverilog/register";
import "monaco-editor/languages/definitions/markdown/register";
import "monaco-editor/languages/definitions/yaml/register";
import "monaco-editor/languages/definitions/tcl/register";
import "monaco-editor/languages/definitions/python/register";
import "monaco-editor/languages/definitions/ini/register";
import "monaco-editor/languages/definitions/shell/register";
import "monaco-editor/language/json/monaco.contribution";
import EditorWorker from "monaco-editor/editor/editor.worker?worker";
import JsonWorker from "monaco-editor/language/json/json.worker?worker";
import { loader } from "@monaco-editor/react";

self.MonacoEnvironment = {
  getWorker: (_id, label) => (label === "json" ? new JsonWorker() : new EditorWorker()),
};
loader.config({ monaco });

const byExtension: Record<string, string> = {
  v: "verilog",
  vh: "verilog",
  sv: "systemverilog",
  svh: "systemverilog",
  json: "json",
  md: "markdown",
  yaml: "yaml",
  yml: "yaml",
  tcl: "tcl",
  sdc: "tcl",
  py: "python",
  toml: "ini",
  sh: "shell",
};

export function languageFor(path: string): string {
  const ext = path.split(".").pop()?.toLowerCase() ?? "";
  return byExtension[ext] ?? "plaintext";
}
