import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it } from "vitest";

import App from "../src/renderer/App";
import { getBundledAppearance } from "../src/renderer/appearance/AppearanceProvider";
import { themeCssVariables } from "../src/renderer/appearance/themePalette";

describe("desktop appearance", () => {
  afterEach(() => {
    cleanup();
    window.localStorage.clear();
    window.history.replaceState(null, "", "/");
    document.documentElement.removeAttribute("data-appearance");
    document.documentElement.removeAttribute("data-high-contrast");
    document.documentElement.removeAttribute("data-theme-id");
    document.documentElement.removeAttribute("style");
  });

  it("packages the current VS Code theme set and defaults to Dark 2026", () => {
    const bundle = getBundledAppearance();

    expect(bundle.defaultTheme).toBe("vscode.dark-2026");
    expect(bundle.themes).toHaveLength(19);
    expect(bundle.themes.map((theme) => theme.label)).toEqual(
      expect.arrayContaining([
        "Dark 2026",
        "Light 2026",
        "Dark Modern",
        "Monokai",
        "Solarized Dark",
        "Tomorrow Night Blue"
      ])
    );
  });

  it("maps VS Code workbench colors onto App surface tokens", () => {
    const theme = getBundledAppearance().themes.find(
      (candidate) => candidate.id === "vscode.dark-2026"
    )!;

    expect(themeCssVariables(theme)).toMatchObject({
      "--color-background": "#121314",
      "--color-primary-fill": "#297AA0",
      "--color-surface": "#191A1B",
      "--color-text": "#bfbfbf",
      "--color-user-message": "#ffffff13"
    });
  });

  it("switches themes immediately and persists the selection", async () => {
    const user = userEvent.setup();
    window.history.replaceState(null, "", "/#settings");
    render(<App />);

    await user.click(screen.getByRole("radio", { name: /Light 2026/ }));

    await waitFor(() => {
      expect(document.documentElement).toHaveAttribute("data-theme-id", "vscode.light-2026");
    });
    expect(document.documentElement).toHaveAttribute("data-appearance", "light");
    expect(document.documentElement.style.getPropertyValue("--color-background")).toBe("#FFFFFF");
    expect(window.localStorage.getItem("generalagent.appearance.theme.v1")).toBe(
      "vscode.light-2026"
    );
  });
});
