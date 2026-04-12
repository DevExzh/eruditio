<script lang="ts">
  let {
    message,
    onRetry,
  }: {
    message: string;
    onRetry: () => void;
  } = $props();
</script>

<div class="error">
  <div class="message">⚠ {message}</div>
  <button onclick={onRetry}>Try again</button>
</div>

<style>
  .error {
    margin-top: 20px;
    background: #1c1917;
    border: 1px solid var(--error);
    border-radius: 8px;
    padding: 16px;
    text-align: center;
  }
  .message {
    font-size: 14px;
    color: var(--error);
    margin-bottom: 12px;
    word-break: break-word;
  }
  button {
    background: none;
    border: 1px solid var(--error);
    color: var(--error);
    padding: 6px 16px;
    border-radius: 6px;
    font-size: 13px;
    cursor: pointer;
  }
  button:hover { background: var(--error); color: white; }
</style>
