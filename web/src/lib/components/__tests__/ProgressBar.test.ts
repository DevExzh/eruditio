import { render, screen } from "@testing-library/svelte";
import { describe, expect, test } from "vitest";
import ProgressBar from "../ProgressBar.svelte";

describe("ProgressBar", () => {
  test("renders the stage text", () => {
    render(ProgressBar, { props: { stage: "Reading..." } });
    expect(screen.getByText("Reading...")).toBeTruthy();
  });

  test("shows Step 1 of 4 for Reading stage", () => {
    render(ProgressBar, { props: { stage: "Reading..." } });
    expect(screen.getByText("Step 1 of 4")).toBeTruthy();
  });

  test("shows Step 2 of 4 for Transforming stage", () => {
    render(ProgressBar, { props: { stage: "Transforming..." } });
    expect(screen.getByText("Step 2 of 4")).toBeTruthy();
  });

  test("shows Step 3 of 4 for Writing stage", () => {
    render(ProgressBar, { props: { stage: "Writing..." } });
    expect(screen.getByText("Step 3 of 4")).toBeTruthy();
  });

  test("shows Step 4 of 4 for Done stage", () => {
    render(ProgressBar, { props: { stage: "Done" } });
    expect(screen.getByText("Step 4 of 4")).toBeTruthy();
  });

  test("computes 25% progress for Reading stage", () => {
    render(ProgressBar, { props: { stage: "Reading..." } });
    const fill = document.querySelector(".fill") as HTMLElement;
    expect(fill.style.width).toBe("25%");
  });

  test("computes 50% progress for Transforming stage", () => {
    render(ProgressBar, { props: { stage: "Transforming..." } });
    const fill = document.querySelector(".fill") as HTMLElement;
    expect(fill.style.width).toBe("50%");
  });

  test("computes 75% progress for Writing stage", () => {
    render(ProgressBar, { props: { stage: "Writing..." } });
    const fill = document.querySelector(".fill") as HTMLElement;
    expect(fill.style.width).toBe("75%");
  });

  test("computes 100% progress for Done stage", () => {
    render(ProgressBar, { props: { stage: "Done" } });
    const fill = document.querySelector(".fill") as HTMLElement;
    expect(fill.style.width).toBe("100%");
  });

  test("computes 0% progress for unknown stage", () => {
    render(ProgressBar, { props: { stage: "Unknown" } });
    const fill = document.querySelector(".fill") as HTMLElement;
    expect(fill.style.width).toBe("0%");
  });
});
