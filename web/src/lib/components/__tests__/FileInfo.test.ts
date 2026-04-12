import { render, screen, fireEvent } from "@testing-library/svelte";
import { describe, expect, test, vi } from "vitest";
import FileInfo from "../FileInfo.svelte";

describe("FileInfo", () => {
  const defaultProps = {
    name: "test-book.epub",
    size: 1024,
    format: "epub",
    onClear: vi.fn(),
  };

  test("renders the file name", () => {
    render(FileInfo, { props: defaultProps });
    expect(screen.getByText(/test-book\.epub/)).toBeTruthy();
  });

  test("renders the format in uppercase", () => {
    render(FileInfo, { props: defaultProps });
    expect(screen.getByText(/EPUB detected/)).toBeTruthy();
  });

  test("renders the clear button", () => {
    render(FileInfo, { props: defaultProps });
    expect(screen.getByText(/Clear/)).toBeTruthy();
  });

  test("calls onClear when clear button is clicked", async () => {
    const onClear = vi.fn();
    render(FileInfo, { props: { ...defaultProps, onClear } });
    await fireEvent.click(screen.getByText(/Clear/));
    expect(onClear).toHaveBeenCalled();
  });

  test("formats bytes correctly", () => {
    render(FileInfo, { props: { ...defaultProps, size: 500 } });
    expect(screen.getByText(/500 B/)).toBeTruthy();
  });

  test("formats kilobytes correctly", () => {
    render(FileInfo, { props: { ...defaultProps, size: 2048 } });
    expect(screen.getByText(/2\.0 KB/)).toBeTruthy();
  });

  test("formats megabytes correctly", () => {
    render(FileInfo, { props: { ...defaultProps, size: 5 * 1024 * 1024 } });
    expect(screen.getByText(/5\.0 MB/)).toBeTruthy();
  });
});
