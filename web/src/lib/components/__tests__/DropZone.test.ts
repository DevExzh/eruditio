import { render, screen, fireEvent } from "@testing-library/svelte";
import { describe, expect, test, vi } from "vitest";
import DropZone from "../DropZone.svelte";

describe("DropZone", () => {
  test("renders the drop zone text", () => {
    const onFile = vi.fn();
    render(DropZone, { props: { onFile } });
    expect(screen.getByText(/Drop your ebook here/)).toBeTruthy();
  });

  test("renders the browse files link text", () => {
    const onFile = vi.fn();
    render(DropZone, { props: { onFile } });
    expect(screen.getByText("browse files")).toBeTruthy();
  });

  test("renders the supported formats hint", () => {
    const onFile = vi.fn();
    render(DropZone, { props: { onFile } });
    expect(screen.getByText(/Supports EPUB, MOBI/)).toBeTruthy();
  });

  test("has a button role for accessibility", () => {
    const onFile = vi.fn();
    render(DropZone, { props: { onFile } });
    expect(screen.getByRole("button")).toBeTruthy();
  });

  test("calls onFile when a file is selected via the hidden input", async () => {
    const onFile = vi.fn();
    render(DropZone, { props: { onFile } });

    const input = document.querySelector('input[type="file"]') as HTMLInputElement;
    expect(input).toBeTruthy();

    const file = new File(["hello"], "test.epub", { type: "application/epub+zip" });
    Object.defineProperty(input, "files", { value: [file], writable: false });

    await fireEvent.input(input);
    expect(onFile).toHaveBeenCalledWith(file);
  });

  test("calls onFile when a file is dropped", async () => {
    const onFile = vi.fn();
    render(DropZone, { props: { onFile } });

    const dropzone = screen.getByRole("button");
    const file = new File(["content"], "book.mobi", { type: "application/x-mobipocket-ebook" });

    await fireEvent.drop(dropzone, {
      dataTransfer: { files: [file] },
    });
    expect(onFile).toHaveBeenCalledWith(file);
  });
});
