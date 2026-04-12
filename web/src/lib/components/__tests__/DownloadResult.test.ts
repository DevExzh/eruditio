import { render, screen, fireEvent } from "@testing-library/svelte";
import { describe, expect, test, vi, beforeEach } from "vitest";
import DownloadResult from "../DownloadResult.svelte";

describe("DownloadResult", () => {
  beforeEach(() => {
    globalThis.URL.createObjectURL = vi.fn(() => "blob:mock-url");
    globalThis.URL.revokeObjectURL = vi.fn();
  });

  const defaultProps = {
    filename: "output.epub",
    blob: new Blob(["test content"], { type: "application/epub+zip" }),
    onReset: vi.fn(),
  };

  test("renders the success message", () => {
    render(DownloadResult, { props: defaultProps });
    expect(screen.getByText(/Conversion complete/)).toBeTruthy();
  });

  test("renders the download link with filename", () => {
    render(DownloadResult, { props: defaultProps });
    const link = screen.getByText(/Download output\.epub/) as HTMLAnchorElement;
    expect(link).toBeTruthy();
    expect(link.tagName).toBe("A");
  });

  test("download link has correct download attribute", () => {
    render(DownloadResult, { props: defaultProps });
    const link = screen.getByText(/Download output\.epub/) as HTMLAnchorElement;
    expect(link.getAttribute("download")).toBe("output.epub");
  });

  test("download link has a blob URL href", () => {
    render(DownloadResult, { props: defaultProps });
    const link = screen.getByText(/Download output\.epub/) as HTMLAnchorElement;
    expect(link.href).toContain("blob:");
  });

  test("renders the Convert another file button", () => {
    render(DownloadResult, { props: defaultProps });
    expect(screen.getByText("Convert another file")).toBeTruthy();
  });

  test("calls onReset when Convert another file is clicked", async () => {
    const onReset = vi.fn();
    render(DownloadResult, { props: { ...defaultProps, onReset } });
    await fireEvent.click(screen.getByText("Convert another file"));
    expect(onReset).toHaveBeenCalled();
  });
});
