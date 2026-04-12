/// <reference lib="webworker" />
import type { WorkerRequest } from "./types";
import init, {
  convert,
  supported_formats,
} from "./wasm/eruditio_wasm";

self.onmessage = async (e: MessageEvent<WorkerRequest>) => {
  switch (e.data.type) {
    case "init": {
      try {
        await init();
        self.postMessage({ type: "ready" });
      } catch (err) {
        self.postMessage({
          type: "error",
          message: `Failed to load WASM: ${err}`,
        });
      }
      break;
    }
    case "convert": {
      const { input, inputFormat, outputFormat, options } = e.data.payload;
      try {
        const result = convert(
          new Uint8Array(input),
          inputFormat,
          outputFormat,
          options,
          (stage: string) => self.postMessage({ type: "progress", stage }),
        );
        const buffer = result.buffer;
        self.postMessage(
          {
            type: "result",
            output: buffer,
            filename: `converted.${outputFormat}`,
          },
          [buffer],
        );
      } catch (err) {
        self.postMessage({ type: "error", message: String(err) });
      }
      break;
    }
    case "list-formats": {
      try {
        const formats = supported_formats();
        self.postMessage({ type: "formats", ...formats });
      } catch (err) {
        self.postMessage({
          type: "error",
          message: `Failed to list formats: ${err}`,
        });
      }
      break;
    }
  }
};
