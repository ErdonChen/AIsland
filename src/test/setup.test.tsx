import { render, screen } from "@testing-library/react";
import { expect, test } from "vitest";

test("the test harness renders jsdom and provides jest-dom matchers", () => {
  render(<p>AIceLand test harness</p>);

  expect(screen.getByText("AIceLand test harness")).toBeInTheDocument();
});
