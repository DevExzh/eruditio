<script lang="ts">
  let {
    filename,
    blob,
    onReset,
  }: {
    filename: string;
    blob: Blob;
    onReset: () => void;
  } = $props();

  let url = $derived(URL.createObjectURL(blob));
</script>

<div class="result">
  <div class="success">✓ Conversion complete</div>
  <a class="download" href={url} download={filename}>
    Download {filename}
  </a>
  <button class="another" onclick={onReset}>Convert another file</button>
</div>

<style>
  .result {
    margin-top: 20px;
    background: var(--bg-card);
    border: 1px solid var(--success);
    border-radius: 8px;
    padding: 20px;
    text-align: center;
  }
  .success {
    font-size: 16px;
    color: var(--success);
    margin-bottom: 16px;
  }
  .download {
    display: inline-block;
    background: var(--success);
    color: white;
    padding: 10px 24px;
    border-radius: 8px;
    text-decoration: none;
    font-weight: 600;
    font-size: 14px;
  }
  .download:hover { opacity: 0.9; }
  .another {
    display: block;
    margin: 12px auto 0;
    background: none;
    border: none;
    color: var(--text-muted);
    font-size: 13px;
    cursor: pointer;
    text-decoration: underline;
  }
</style>
