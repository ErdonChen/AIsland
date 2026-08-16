import { cleanup, render, screen, within } from "@testing-library/react";
import { afterEach, expect, test } from "vitest";
import { I18nProvider } from "../i18n/I18nProvider";
import MetricTrend from "./MetricTrend";

function renderTrend(points: Array<{ sampledAt: number; value: number }>) {
  return render(<I18nProvider><MetricTrend label="CPU" unit="%" points={points} /></I18nProvider>);
}

afterEach(cleanup);

test("renders a labelled trend with textual min current and max values", () => {
  renderTrend([
    { sampledAt: 1, value: 10 },
    { sampledAt: 2, value: 35.25 },
    { sampledAt: 3, value: 20 },
  ]);
  const figure = screen.getByRole("figure", { name: "CPU — 最近 15 分钟" });
  expect(figure).toHaveTextContent("最低10.0%");
  expect(figure).toHaveTextContent("当前20.0%");
  expect(figure).toHaveTextContent("最高35.3%");
  expect(within(figure).getByRole("img", { name: "CPU trend" })).toBeInTheDocument();
});

test("handles empty one-point equal and invalid series without invalid geometry", () => {
  const { rerender } = renderTrend([]);
  expect(screen.getByText("等待首个有效采样")).toBeInTheDocument();

  rerender(<I18nProvider><MetricTrend label="CPU" unit="%" points={[{ sampledAt: 1, value: 12 }]} /></I18nProvider>);
  expect(document.querySelectorAll("circle")).toHaveLength(1);

  rerender(<I18nProvider><MetricTrend label="CPU" unit="%" points={[
    { sampledAt: 1, value: Number.NaN },
    { sampledAt: 2, value: -1 },
    { sampledAt: 3, value: 5 },
    { sampledAt: 4, value: 5 },
  ]} /></I18nProvider>);
  expect(document.querySelector("polyline")?.getAttribute("points")).not.toMatch(/NaN|Infinity/);
  expect(screen.queryByText("-1.0%")).not.toBeInTheDocument();
});
