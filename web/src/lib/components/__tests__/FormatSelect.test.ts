import { render, screen } from "@testing-library/svelte";
import { describe, expect, test } from "vitest";
import FormatSelect from "../FormatSelect.svelte";

describe("FormatSelect", () => {
  const formats = ["epub", "mobi", "pdf", "txt"];

  test("renders the label", () => {
    render(FormatSelect, { props: { formats, value: "epub" } });
    expect(screen.getByText("Output format")).toBeTruthy();
  });

  test("renders a select element", () => {
    render(FormatSelect, { props: { formats, value: "epub" } });
    const select = screen.getByRole("combobox") as HTMLSelectElement;
    expect(select).toBeTruthy();
  });

  test("renders all format options", () => {
    render(FormatSelect, { props: { formats, value: "epub" } });
    const options = screen.getAllByRole("option");
    expect(options.length).toBe(4);
  });

  test("renders options in uppercase", () => {
    render(FormatSelect, { props: { formats, value: "epub" } });
    expect(screen.getByText("EPUB")).toBeTruthy();
    expect(screen.getByText("MOBI")).toBeTruthy();
    expect(screen.getByText("PDF")).toBeTruthy();
    expect(screen.getByText("TXT")).toBeTruthy();
  });

  test("has the correct initial value selected", () => {
    render(FormatSelect, { props: { formats, value: "mobi" } });
    const select = screen.getByRole("combobox") as HTMLSelectElement;
    expect(select.value).toBe("mobi");
  });
});
