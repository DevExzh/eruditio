export interface ConversionOptions {
  title?: string;
  authors?: string;
  publisher?: string;
  language?: string;
  isbn?: string;
  description?: string;
  series?: string;
  seriesIndex?: number;
  tags?: string;
  rights?: string;
}

export type WorkerRequest =
  | { type: "init" }
  | {
      type: "convert";
      payload: {
        input: ArrayBuffer;
        inputFormat: string;
        outputFormat: string;
        options: ConversionOptions;
      };
    }
  | { type: "list-formats" };

export type WorkerResponse =
  | { type: "ready" }
  | { type: "progress"; stage: string }
  | { type: "result"; output: ArrayBuffer; filename: string }
  | { type: "error"; message: string }
  | { type: "formats"; input: string[]; output: string[] };

export type AppState = "idle" | "file-loaded" | "converting" | "done" | "error";
