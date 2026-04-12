import { render, screen } from "@testing-library/svelte";
import { describe, expect, test } from "vitest";
import Header from "../Header.svelte";

describe("Header", () => {
  test("renders the title", () => {
    render(Header);
    expect(screen.getByText("eruditio")).toBeTruthy();
  });

  test("renders the tagline", () => {
    render(Header);
    expect(
      screen.getByText("ebook converter — runs entirely in your browser"),
    ).toBeTruthy();
  });

  test("title is an h1 element", () => {
    render(Header);
    const heading = screen.getByRole("heading", { level: 1 });
    expect(heading.textContent).toBe("eruditio");
  });
});
