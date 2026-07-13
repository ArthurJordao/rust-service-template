import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, it, vi } from "vitest";
import { RecoveryCodesDialog } from "./RecoveryCodesDialog";

it("gates Done behind the acknowledgment", async () => {
  const onDone = vi.fn();
  render(<RecoveryCodesDialog codes={["aaaaa-bbbbb", "ccccc-ddddd"]} open onDone={onDone} />);
  const done = screen.getByRole("button", { name: /done/i });
  expect(done).toBeDisabled();
  await userEvent.click(screen.getByRole("checkbox"));
  expect(done).toBeEnabled();
  await userEvent.click(done);
  expect(onDone).toHaveBeenCalledOnce();
});
