import { render, screen, fireEvent } from "@testing-library/svelte";
import { describe, expect, test } from "vitest";
import MetadataEditor from "../MetadataEditor.svelte";

describe("MetadataEditor", () => {
  test("renders the toggle button", () => {
    render(MetadataEditor, { props: { options: {} } });
    expect(screen.getByText("Metadata overrides")).toBeTruthy();
  });

  test("renders the optional hint", () => {
    render(MetadataEditor, { props: { options: {} } });
    expect(screen.getByText("(optional)")).toBeTruthy();
  });

  test("fields are hidden by default", () => {
    render(MetadataEditor, { props: { options: {} } });
    expect(document.querySelector(".fields")).toBeNull();
  });

  test("shows right arrow when collapsed", () => {
    render(MetadataEditor, { props: { options: {} } });
    expect(screen.getByText("▶")).toBeTruthy();
  });

  test("shows fields when toggle is clicked", async () => {
    render(MetadataEditor, { props: { options: {} } });
    await fireEvent.click(screen.getByText("Metadata overrides"));
    expect(document.querySelector(".fields")).toBeTruthy();
  });

  test("shows down arrow when expanded", async () => {
    render(MetadataEditor, { props: { options: {} } });
    await fireEvent.click(screen.getByText("Metadata overrides"));
    expect(screen.getByText("▼")).toBeTruthy();
  });

  test("hides fields when toggle is clicked again", async () => {
    render(MetadataEditor, { props: { options: {} } });
    const toggle = screen.getByText("Metadata overrides");
    await fireEvent.click(toggle);
    expect(document.querySelector(".fields")).toBeTruthy();
    await fireEvent.click(toggle);
    expect(document.querySelector(".fields")).toBeNull();
  });

  test("shows Title input field when expanded", async () => {
    render(MetadataEditor, { props: { options: {} } });
    await fireEvent.click(screen.getByText("Metadata overrides"));
    expect(screen.getByLabelText("Title")).toBeTruthy();
  });

  test("shows Authors input field when expanded", async () => {
    render(MetadataEditor, { props: { options: {} } });
    await fireEvent.click(screen.getByText("Metadata overrides"));
    expect(screen.getByLabelText("Authors")).toBeTruthy();
  });

  test("shows Publisher input field when expanded", async () => {
    render(MetadataEditor, { props: { options: {} } });
    await fireEvent.click(screen.getByText("Metadata overrides"));
    expect(screen.getByLabelText("Publisher")).toBeTruthy();
  });

  test("shows Language input field when expanded", async () => {
    render(MetadataEditor, { props: { options: {} } });
    await fireEvent.click(screen.getByText("Metadata overrides"));
    expect(screen.getByLabelText("Language")).toBeTruthy();
  });

  test("shows Series input field when expanded", async () => {
    render(MetadataEditor, { props: { options: {} } });
    await fireEvent.click(screen.getByText("Metadata overrides"));
    expect(screen.getByLabelText("Series")).toBeTruthy();
  });

  test("shows Description textarea when expanded", async () => {
    render(MetadataEditor, { props: { options: {} } });
    await fireEvent.click(screen.getByText("Metadata overrides"));
    expect(screen.getByLabelText("Description")).toBeTruthy();
  });
});
