import { useState } from "react";
import type { Meta, StoryObj } from "@storybook/react-vite";
import { expect, userEvent, within, waitFor } from "storybook/test";
import { Button } from "./button";
import { Dialog, DialogActions, DialogBody } from "./dialog";
import { Alert, AlertActions, AlertDescription, AlertTitle } from "./alert";
import { Text } from "./text";
import { DialogHeader } from "../ui/DialogHeader";

const meta = { title: "Catalyst/Диалоги", tags: ["autodocs"] } satisfies Meta;
export default meta;
type Story = StoryObj<typeof meta>;
function DialogExample({
  long = false,
  busy = false,
}: {
  long?: boolean;
  busy?: boolean;
}) {
  const [open, setOpen] = useState(false);
  return (
    <>
      <Button onClick={() => setOpen(true)}>Открыть диалог</Button>
      <Dialog
        open={open}
        onClose={() => {
          if (!busy) setOpen(false);
        }}
      >
        <DialogHeader
          title="Установка агента"
          onClose={() => setOpen(false)}
          disabled={busy}
        />
        <DialogBody>
          {(long ? Array.from({ length: 12 }, (_, i) => i) : [0]).map((i) => (
            <Text key={i} className="mt-3">
              Wizard сохранит настройки приложения. Длинное описание переносится
              внутри диалога и остаётся доступным при прокрутке.
            </Text>
          ))}
        </DialogBody>
        <DialogActions>
          <Button plain disabled={busy} onClick={() => setOpen(false)}>
            Назад
          </Button>
          <Button disabled={busy} onClick={() => setOpen(false)}>
            {busy ? "Устанавливаем…" : "Установить"}
          </Button>
        </DialogActions>
      </Dialog>
    </>
  );
}
export const Standard: Story = {
  name: "Dialog и клавиатура",
  render: () => <DialogExample />,
  play: async ({ canvas, canvasElement }) => {
    const trigger = canvas.getByRole("button", { name: "Открыть диалог" });
    await userEvent.click(trigger);
    const page = within(canvasElement.ownerDocument.body);
    await expect(await page.findByRole("dialog")).toBeVisible();
    await userEvent.keyboard("{Escape}");
    await waitFor(() =>
      expect(page.queryByRole("dialog")).not.toBeInTheDocument(),
    );
    await expect(trigger).toHaveFocus();
  },
};
export const LongContent: Story = {
  name: "Длинное содержимое",
  render: () => <DialogExample long />,
  play: async ({ canvas }) => {
    await userEvent.click(
      canvas.getByRole("button", { name: "Открыть диалог" }),
    );
  },
};
export const Busy: Story = {
  name: "Недоступное закрытие во время операции",
  render: () => <DialogExample busy />,
  play: async ({ canvas, canvasElement }) => {
    await userEvent.click(
      canvas.getByRole("button", { name: "Открыть диалог" }),
    );
    const page = within(canvasElement.ownerDocument.body);
    await page.findByRole("dialog");
    await userEvent.keyboard("{Escape}");
    await expect(page.getByRole("dialog")).toBeVisible();
    await expect(page.getByRole("button", { name: "Закрыть" })).toBeDisabled();
  },
};
function AlertExample() {
  const [open, setOpen] = useState(false);
  return (
    <>
      <Button outline onClick={() => setOpen(true)}>
        Показать подтверждение
      </Button>
      <Alert open={open} onClose={setOpen}>
        <AlertTitle>Отменить загрузку?</AlertTitle>
        <AlertDescription>
          Скачанная часть будет удалена. Позже можно начать загрузку заново.
        </AlertDescription>
        <AlertActions>
          <Button plain onClick={() => setOpen(false)}>
            Назад
          </Button>
          <Button onClick={() => setOpen(false)}>Отменить загрузку</Button>
        </AlertActions>
      </Alert>
    </>
  );
}
export const Confirmation: Story = {
  name: "Alert",
  render: () => <AlertExample />,
};
