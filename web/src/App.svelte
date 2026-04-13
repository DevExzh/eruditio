<script lang="ts">
  import type { AppState, ConversionOptions, WorkerResponse } from "./lib/types";
  import Header from "./lib/components/Header.svelte";
  import DropZone from "./lib/components/DropZone.svelte";
  import FileInfo from "./lib/components/FileInfo.svelte";
  import FormatSelect from "./lib/components/FormatSelect.svelte";
  import MetadataEditor from "./lib/components/MetadataEditor.svelte";
  import ConvertButton from "./lib/components/ConvertButton.svelte";
  import ProgressBar from "./lib/components/ProgressBar.svelte";
  import ErrorDisplay from "./lib/components/ErrorDisplay.svelte";
  import DownloadResult from "./lib/components/DownloadResult.svelte";

  // State
  let state: AppState = $state("idle");
  let file: File | null = $state(null);
  let inputFormat = $state("");
  let outputFormat = $state("epub");
  let outputFormats: string[] = $state([]);
  let metadataOptions: ConversionOptions = $state({});
  let progressStage = $state("");
  let errorMessage = $state("");
  let resultBlob: Blob | null = $state(null);
  let resultFilename = $state("");
  let workerReady = $state(false);

  // Worker setup
  const worker = new Worker(
    new URL("./lib/worker.ts", import.meta.url),
    { type: "module" },
  );

  worker.onmessage = (e: MessageEvent<WorkerResponse>) => {
    switch (e.data.type) {
      case "ready":
        workerReady = true;
        worker.postMessage({ type: "list-formats" });
        break;
      case "formats":
        outputFormats = e.data.output;
        break;
      case "progress":
        progressStage = e.data.stage;
        break;
      case "result":
        resultBlob = new Blob([e.data.output]);
        resultFilename = e.data.filename;
        state = "done";
        break;
      case "error":
        errorMessage = e.data.message;
        state = "error";
        break;
    }
  };

  worker.postMessage({ type: "init" });

  // Actions
  function handleFile(f: File) {
    file = f;
    const ext = f.name.split(".").pop()?.toLowerCase() ?? "";
    inputFormat = ext;
    state = "file-loaded";
  }

  function handleClear() {
    file = null;
    inputFormat = "";
    metadataOptions = {};
    state = "idle";
  }

  async function handleConvert() {
    if (!file) return;
    state = "converting";
    progressStage = "Starting...";

    const buffer = await file.arrayBuffer();
    worker.postMessage(
      {
        type: "convert",
        payload: {
          input: buffer,
          inputFormat,
          outputFormat,
          options: $state.snapshot(metadataOptions),
        },
      },
      [buffer],
    );
  }

  function handleReset() {
    file = null;
    inputFormat = "";
    resultBlob = null;
    resultFilename = "";
    metadataOptions = {};
    state = "idle";
  }

  function handleRetry() {
    state = "file-loaded";
  }
</script>

<Header />

<main>
  {#if state === "idle"}
    <DropZone onFile={handleFile} />
    {#if !workerReady}
      <p class="loading">Loading converter engine...</p>
    {/if}
  {/if}

  {#if state === "file-loaded" && file}
    <FileInfo
      name={file.name}
      size={file.size}
      format={inputFormat}
      onClear={handleClear}
    />
    <FormatSelect formats={outputFormats} bind:value={outputFormat} />
    <MetadataEditor bind:options={metadataOptions} />
    <ConvertButton
      format={outputFormat}
      disabled={!workerReady}
      onclick={handleConvert}
    />
  {/if}

  {#if state === "converting"}
    <ProgressBar stage={progressStage} />
  {/if}

  {#if state === "done" && resultBlob}
    <DownloadResult
      filename={resultFilename}
      blob={resultBlob}
      onReset={handleReset}
    />
  {/if}

  {#if state === "error"}
    <ErrorDisplay message={errorMessage} onRetry={handleRetry} />
  {/if}
</main>

<style>
  main {
    max-width: 600px;
    margin: 0 auto;
    padding: 32px 24px;
  }
  .loading {
    text-align: center;
    color: var(--text-dim);
    font-size: 13px;
    margin-top: 12px;
  }
</style>
