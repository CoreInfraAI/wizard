import { useState } from "react";
import type { Meta, StoryObj } from "@storybook/react-vite";
import { expect, userEvent, within } from "storybook/test";
import { ChevronDownIcon } from "@heroicons/react/20/solid";
import {
  Dropdown,
  DropdownButton,
  DropdownDescription,
  DropdownDivider,
  DropdownItem,
  DropdownLabel,
  DropdownMenu,
} from "./dropdown";
import {
  Listbox,
  ListboxDescription,
  ListboxLabel,
  ListboxOption,
} from "./listbox";
import {
  Combobox,
  ComboboxDescription,
  ComboboxLabel,
  ComboboxOption,
} from "./combobox";
import { Field, Label } from "./fieldset";
import { Text } from "./text";

const meta = {
  title: "Catalyst/Меню",
  tags: ["autodocs"],
  decorators: [
    (Story) => (
      <div className="max-w-sm">
        <Story />
      </div>
    ),
  ],
} satisfies Meta;
export default meta;
type Story = StoryObj<typeof meta>;
function DropdownExample() {
  const [selected, setSelected] = useState("Не выбрано");
  return (
    <>
      <Dropdown>
        <DropdownButton outline>
          Действия
          <ChevronDownIcon />
        </DropdownButton>
        <DropdownMenu anchor="bottom start">
          <DropdownItem onClick={() => setSelected("Настройки")}>
            <DropdownLabel>Настройки</DropdownLabel>
            <DropdownDescription>Параметры приложения</DropdownDescription>
          </DropdownItem>
          <DropdownItem onClick={() => setSelected("Логи")}>
            <DropdownLabel>Логи</DropdownLabel>
          </DropdownItem>
          <DropdownDivider />
          <DropdownItem disabled>
            <DropdownLabel>Недоступное действие</DropdownLabel>
          </DropdownItem>
        </DropdownMenu>
      </Dropdown>
      <Text className="mt-4">{selected}</Text>
    </>
  );
}
export const Menu: Story = {
  name: "Dropdown",
  render: () => <DropdownExample />,
  play: async ({ canvas, canvasElement }) => {
    await userEvent.click(canvas.getByRole("button", { name: "Действия" }));
    const page = within(canvasElement.ownerDocument.body);
    await userEvent.click(
      await page.findByRole("menuitem", { name: /Настройки/ }),
    );
    await expect(canvas.getByText("Настройки")).toBeVisible();
  },
};
const hubs = [
  { id: "personal", name: "Личный хаб", description: "Только для вас" },
  {
    id: "team",
    name: "Команда разработки и исследования интерфейсов",
    description: "Общий доступ команды",
  },
];
function ListboxExample() {
  const [value, setValue] = useState(hubs[0]);
  return (
    <Field>
      <Label>Хаб</Label>
      <Listbox value={value} onChange={setValue}>
        {hubs.map((hub) => (
          <ListboxOption key={hub.id} value={hub}>
            <ListboxLabel>{hub.name}</ListboxLabel>
            <ListboxDescription>{hub.description}</ListboxDescription>
          </ListboxOption>
        ))}
      </Listbox>
    </Field>
  );
}
export const Selection: Story = {
  name: "Listbox",
  render: () => <ListboxExample />,
  play: async ({ canvas, canvasElement }) => {
    const control = canvas.getByRole("button", { name: /Хаб/ });
    await userEvent.click(control);
    const page = within(canvasElement.ownerDocument.body);
    await userEvent.click(
      await page.findByRole("option", { name: /Команда разработки/ }),
    );
    await expect(control).toHaveTextContent(hubs[1].name);
  },
};
function ComboboxExample() {
  const [value, setValue] = useState<(typeof hubs)[number] | null>(null);
  return (
    <Field>
      <Label>Найти хаб</Label>
      <Combobox<(typeof hubs)[number] | null>
        options={hubs}
        value={value}
        onChange={setValue}
        displayValue={(hub) => hub?.name}
        placeholder="Название хаба"
      >
        {(hub) => (
          <ComboboxOption value={hub}>
            <ComboboxLabel>{hub.name}</ComboboxLabel>
            <ComboboxDescription>{hub.description}</ComboboxDescription>
          </ComboboxOption>
        )}
      </Combobox>
    </Field>
  );
}
export const SearchSelection: Story = {
  name: "Combobox",
  // Headless UI virtualizes options using measured viewport geometry.
  parameters: { browserOnly: true },
  render: () => <ComboboxExample />,
  play: async ({ canvas, canvasElement }) => {
    const control = canvas.getByRole("combobox", { name: "Найти хаб" });
    await userEvent.type(control, "Команда");
    const page = within(canvasElement.ownerDocument.body);
    await userEvent.click(
      await page.findByRole("option", { name: /Команда разработки/ }),
    );
    await expect(control).toHaveValue(hubs[1].name);
  },
};
