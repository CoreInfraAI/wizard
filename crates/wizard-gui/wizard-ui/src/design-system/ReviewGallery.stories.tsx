import type { Meta, StoryObj } from "@storybook/react-vite";
import { useState } from "react";
import { CheckIcon, MagnifyingGlassIcon } from "@heroicons/react/20/solid";
import { Button } from "../components/catalyst/button";
import { Badge } from "../components/catalyst/badge";
import {
  Field,
  Label,
  Description,
  ErrorMessage,
} from "../components/catalyst/fieldset";
import { Input, InputGroup } from "../components/catalyst/input";
import { Select } from "../components/catalyst/select";
import { Textarea } from "../components/catalyst/textarea";
import { Checkbox, CheckboxField } from "../components/catalyst/checkbox";
import { Radio, RadioField, RadioGroup } from "../components/catalyst/radio";
import { Switch, SwitchField } from "../components/catalyst/switch";
import { PageHeader, Panel, ActionBar } from "../components/ui/Layout";
import { StateNotice } from "../components/ui/StateNotice";
import { TransferCard } from "../components/ui/TransferCard";
import { ClaudeIcon, OpenAiIcon } from "../components/BrandIcons";

const meta = {
  title: "Начало/Проверка размеров",
  parameters: { layout: "padded" },
} satisfies Meta;
export default meta;
type Story = StoryObj<typeof meta>;

function ReviewGallery() {
  const [mode, setMode] = useState("hub");
  const [enabled, setEnabled] = useState(true);
  const [saved, setSaved] = useState(false);
  return (
    <div className="ui-page ui-stack">
      <PageHeader
        title="Компоненты в одном ритме"
        description="Проверка выравнивания, длинного текста, фокуса и состояний на каждой ширине."
      />
      <Panel title="Действия и состояния">
        <div className="flex flex-wrap items-center gap-3">
          <Button onClick={() => setSaved(true)}>
            <CheckIcon />
            Применить
          </Button>
          <Button outline>Скачать обновление</Button>
          <Button plain>Назад</Button>
          <Button disabled>Устанавливаем…</Button>
          <Badge color="green">Готов</Badge>
        </div>
        {saved && (
          <div className="mt-4">
            <StateNotice tone="success">Настройки сохранены.</StateNotice>
          </div>
        )}
      </Panel>
      <Panel title="Поля">
        <div className="grid items-start gap-6 sm:grid-cols-2">
          <Field>
            <Label>Название подключения</Label>
            <Input placeholder="Рабочий хаб" />
            <Description>Короткая подпись поля.</Description>
          </Field>
          <Field>
            <Label>Хаб</Label>
            <Select>
              <option>Личный хаб</option>
              <option>Команда разработки интерфейсов</option>
            </Select>
            <Description>Список доступных подключений.</Description>
          </Field>
          <Field>
            <Label>Поиск</Label>
            <InputGroup>
              <MagnifyingGlassIcon />
              <Input type="search" placeholder="Найти агента" />
            </InputGroup>
          </Field>
          <Field>
            <Label>Адрес хаба</Label>
            <Input invalid defaultValue="неверный адрес" />
            <ErrorMessage>Введите полный адрес хаба.</ErrorMessage>
          </Field>
          <Field>
            <Label>Комментарий</Label>
            <Textarea rows={3} />
          </Field>
          <Field disabled>
            <Label>Недоступное поле</Label>
            <Input defaultValue="Личный хаб" />
            <Description>Операция уже выполняется.</Description>
          </Field>
        </div>
      </Panel>
      <div className="grid items-start gap-4 sm:grid-cols-2">
        <Panel title="Выбор">
          <RadioGroup
            className="ui-choice-group"
            aria-label="Режим подключения"
            value={mode}
            onChange={setMode}
          >
            <RadioField className="ui-choice">
              <Radio value="hub" />
              <Label>API-доступ</Label>
              <Description>Подключение через CoreInfra AI Hub.</Description>
            </RadioField>
            <RadioField className="ui-choice">
              <Radio value="direct" />
              <Label>Без CoreInfra AI Hub</Label>
              <Description>
                Использовать собственные настройки агента.
              </Description>
            </RadioField>
          </RadioGroup>
        </Panel>
        <Panel title="Настройки">
          <SwitchField>
            <Label>Работать в трее</Label>
            <Description>
              Wizard продолжит работу после закрытия окна.
            </Description>
            <Switch checked={enabled} onChange={setEnabled} />
          </SwitchField>
          <CheckboxField className="mt-6">
            <Checkbox />
            <Label>Подтверждаю замену приложения</Label>
          </CheckboxField>
          <ActionBar divider>
            <Button>Установить</Button>
            <Button plain>Отменить</Button>
          </ActionBar>
        </Panel>
      </div>
      <div className="grid items-stretch gap-4 sm:grid-cols-2">
        <TransferCard
          name="Claude Desktop"
          brand="claude"
          icon={<ClaudeIcon />}
          status="Скачивается · 38%"
          detail="94 / 248 МБ"
          progress={38}
          downloading
          action={<Button outline>Отменить</Button>}
        />
        <TransferCard
          name="ChatGPT"
          brand="chatgpt"
          icon={<OpenAiIcon />}
          status="Версия 1.2026.321 — готова к установке"
          filled
          action={
            <div className="ui-transfer-action-group">
              <Button>Установить</Button>
              <Button plain>Отменить</Button>
            </div>
          }
        />
      </div>
      <StateNotice tone="error" actions={<Button outline>Повторить</Button>}>
        Не удалось прочитать настройки агента. Длинное сообщение остаётся внутри
        блока.
      </StateNotice>
    </div>
  );
}

export const AllControls: Story = {
  name: "Контрольная сетка",
  render: () => <ReviewGallery />,
};
