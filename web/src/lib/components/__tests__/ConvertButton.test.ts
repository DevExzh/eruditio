import { render, screen, fireEvent } from "@testing-library/svelte";
import { describe, expect, test, vi } from "vitest";
import ConvertButton from "../ConvertButton.svelte";

describe("ConvertButton", () => {
  test("renders the format text in uppercase", () => {
    render(ConvertButton, { props: { format: "epub", onclick: vi.fn() } });
    expect(screen.getByText("Convert to EPUB")).toBeTruthy();
  });

  test("renders a button element", () => {
    render(ConvertButton, { props: { format: "mobi", onclick: vi.fn() } });
    const button = screen.getByRole("button");
    expect(button.textContent).toContain("Convert to MOBI");
  });

  test("button is enabled by default", () => {
    render(ConvertButton, { props: { format: "epub", onclick: vi.fn() } });
    const button = screen.getByRole("button") as HTMLButtonElement;
    expect(button.disabled).toBe(false);
  });

  test("button is disabled when disabled prop is true", () => {
    render(ConvertButton, {
      props: { format: "epub", disabled: true, onclick: vi.fn() },
    });
    const button = screen.getByRole("button") as HTMLButtonElement;
    expect(button.disabled).toBe(true);
  });

  test("calls onclick handler when clicked", async () => {
    const onclick = vi.fn();
    render(ConvertButton, { props: { format: "epub", onclick } });
    await fireEvent.click(screen.getByRole("button"));
    expect(onclick).toHaveBeenCalled();
  });
});
