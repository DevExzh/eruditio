import { render, screen, fireEvent } from "@testing-library/svelte";
import { describe, expect, test, vi } from "vitest";
import ErrorDisplay from "../ErrorDisplay.svelte";

describe("ErrorDisplay", () => {
  test("renders the error message", () => {
    render(ErrorDisplay, {
      props: { message: "Conversion failed", onRetry: vi.fn() },
    });
    expect(screen.getByText(/Conversion failed/)).toBeTruthy();
  });

  test("renders the Try again button", () => {
    render(ErrorDisplay, {
      props: { message: "Something went wrong", onRetry: vi.fn() },
    });
    expect(screen.getByText("Try again")).toBeTruthy();
  });

  test("calls onRetry when Try again is clicked", async () => {
    const onRetry = vi.fn();
    render(ErrorDisplay, {
      props: { message: "Error occurred", onRetry },
    });
    await fireEvent.click(screen.getByText("Try again"));
    expect(onRetry).toHaveBeenCalled();
  });

  test("displays long error messages", () => {
    const longMessage =
      "This is a very long error message that describes in detail what went wrong during the conversion process";
    render(ErrorDisplay, {
      props: { message: longMessage, onRetry: vi.fn() },
    });
    expect(screen.getByText(new RegExp(longMessage))).toBeTruthy();
  });
});
