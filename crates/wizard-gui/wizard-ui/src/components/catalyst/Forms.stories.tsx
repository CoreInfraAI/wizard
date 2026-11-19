import type { Meta, StoryObj } from "@storybook/react-vite";
import { useState } from "react";
import { expect, userEvent } from "storybook/test";
import { Checkbox, CheckboxField, CheckboxGroup } from "./checkbox";
import {
  Description,
  ErrorMessage,
  Field,
  FieldGroup,
  Fieldset,
  Label,
  Legend,
} from "./fieldset";
import { Input, InputGroup } from "./input";
import { Textarea } from "./textarea";
import { Select } from "./select";
import { Radio, RadioField, RadioGroup } from "./radio";
import { Switch, SwitchField, SwitchGroup } from "./switch";
import { MagnifyingGlassIcon } from "@heroicons/react/20/solid";

const meta = {
  title: "Catalyst/Поля и выбор",
  tags: ["autodocs"],
  decorators: [
    (Story) => (
      <div className="max-w-xl">
        <Story />
      </div>
    ),
  ],
} satisfies Meta;
export default meta;
type Story = StoryObj<typeof meta>;
export const TextInput: Story = {
  name: "Текстовое поле",
  render: () => (
    <Field>
      <Label>Название подключения</Label>
      <Input placeholder="Например, рабочий хаб" />
      <Description>
        Короткое название, которое удобно найти в списке.
      </Description>
    </Field>
  ),
  play: async ({ canvas }) => {
    const input = canvas.getByLabelText("Название подключения");
    await userEvent.type(input, "Рабочий хаб");
    await expect(input).toHaveValue("Рабочий хаб");
  },
};
export const InvalidInput: Story = {
  name: "Ошибка поля",
  render: () => (
    <Field>
      <Label>Адрес хаба</Label>
      <Input invalid defaultValue="неверный адрес" />
      <ErrorMessage>Введите адрес в формате https://example.com.</ErrorMessage>
    </Field>
  ),
};
export const DisabledInput: Story = {
  name: "Недоступное поле",
  render: () => (
    <Field disabled>
      <Label>Название</Label>
      <Input value="Личный хаб" readOnly />
      <Description>Настройка недоступна во время установки.</Description>
    </Field>
  ),
};
export const Search: Story = {
  name: "Поле с иконкой",
  render: () => (
    <Field>
      <Label>Найти агента</Label>
      <InputGroup>
        <MagnifyingGlassIcon />
        <Input type="search" placeholder="Название агента" />
      </InputGroup>
    </Field>
  ),
};
export const Multiline: Story = {
  name: "Многострочное поле",
  render: () => (
    <Field>
      <Label>Комментарий</Label>
      <Textarea rows={4} placeholder="Опишите подключение" />
      <Description>Длинный текст остаётся в границах поля.</Description>
    </Field>
  ),
};
export const NativeSelect: Story = {
  name: "Select",
  render: () => (
    <Field>
      <Label>Хаб</Label>
      <Select defaultValue="personal">
        <option value="personal">Личный хаб</option>
        <option value="team">Команда разработки и исследований</option>
        <option disabled>Недоступный хаб</option>
      </Select>
    </Field>
  ),
};
export const Checkboxes: Story = {
  name: "Checkbox",
  render: () => (
    <CheckboxGroup>
      <CheckboxField>
        <Checkbox defaultChecked />
        <Label>Закрыть запущенное приложение</Label>
        <Description>Текущая работа должна быть сохранена.</Description>
      </CheckboxField>
      <CheckboxField>
        <Checkbox />
        <Label>Я понимаю, что версия будет заменена</Label>
      </CheckboxField>
      <CheckboxField disabled>
        <Checkbox />
        <Label>Недоступная настройка</Label>
      </CheckboxField>
    </CheckboxGroup>
  ),
};
function RadioExample() {
  const [value, setValue] = useState("hub");
  return (
    <RadioGroup
      value={value}
      onChange={setValue}
      aria-label="Режим подключения"
      className="ui-choice-group"
    >
      {[
        ["hub", "API-доступ"],
        ["sub", "Своя подписка"],
        ["disabled", "Без CoreInfra"],
      ].map(([id, label]) => (
        <RadioField key={id} className="ui-choice">
          <Radio value={id} />
          <Label>{label}</Label>
          <Description>Описание режима подключения агента.</Description>
        </RadioField>
      ))}
    </RadioGroup>
  );
}
export const Radios: Story = {
  name: "Radio",
  render: () => <RadioExample />,
  play: async ({ canvas }) => {
    await userEvent.click(canvas.getByRole("radio", { name: "Своя подписка" }));
    await expect(
      canvas.getByRole("radio", { name: "Своя подписка" }),
    ).toBeChecked();
  },
};
function SwitchExample() {
  const [value, setValue] = useState(true);
  return (
    <SwitchGroup className="ui-setting-list">
      <SwitchField>
        <Label>Работать в трее</Label>
        <Description>При закрытии окна Wizard продолжает работать.</Description>
        <Switch checked={value} onChange={setValue} />
      </SwitchField>
      <SwitchField disabled>
        <Label>Недоступный переключатель</Label>
        <Switch checked={false} onChange={() => {}} />
      </SwitchField>
    </SwitchGroup>
  );
}
export const Switches: Story = {
  name: "Switch",
  render: () => <SwitchExample />,
  play: async ({ canvas }) => {
    await userEvent.click(
      canvas.getByRole("switch", { name: "Работать в трее" }),
    );
    await expect(
      canvas.getByRole("switch", { name: "Работать в трее" }),
    ).not.toBeChecked();
  },
};
export const FieldComposition: Story = {
  name: "Группа полей",
  render: () => (
    <Fieldset>
      <Legend>Подключение</Legend>
      <Description>
        Все подписи и пояснения связываются с полями через Headless UI.
      </Description>
      <FieldGroup>
        <Field>
          <Label>Название</Label>
          <Input />
        </Field>
        <Field>
          <Label>Описание</Label>
          <Textarea rows={3} />
        </Field>
      </FieldGroup>
    </Fieldset>
  ),
};
