import type { Meta, StoryObj } from "@storybook/react-vite";
import { Badge, BadgeButton } from "./badge";
import { Avatar, AvatarButton } from "./avatar";
import {
  DescriptionDetails,
  DescriptionList,
  DescriptionTerm,
} from "./description-list";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "./table";
import {
  Pagination,
  PaginationGap,
  PaginationList,
  PaginationNext,
  PaginationPage,
  PaginationPrevious,
} from "./pagination";
import { Divider } from "./divider";
import { Code, Strong, Text, TextLink } from "./text";
import { Heading, Subheading } from "./heading";
import { Link } from "./link";
import favicon from "../../assets/coreinfra-favicon.png";

const meta = {
  title: "Catalyst/Данные и текст",
  tags: ["autodocs"],
} satisfies Meta;
export default meta;
type Story = StoryObj<typeof meta>;
export const Statuses: Story = {
  name: "Badge",
  render: () => (
    <div className="flex flex-wrap items-center gap-3">
      <Badge>Остановлен</Badge>
      <Badge color="green">Работает</Badge>
      <Badge color="amber">Состояние недоступно</Badge>
      <Badge color="red">Ошибка</Badge>
      <BadgeButton color="blue" onClick={() => {}}>
        Интерактивная метка
      </BadgeButton>
    </div>
  ),
};
export const Avatars: Story = {
  name: "Avatar",
  render: () => (
    <div className="flex items-center gap-4">
      <Avatar className="size-10" initials="AI" alt="Аккаунт AI" />
      <Avatar className="size-10" src={favicon} alt="CoreInfra" />
      <AvatarButton
        className="size-10"
        initials="TT"
        aria-label="Открыть аккаунт"
      />
    </div>
  ),
};
export const Details: Story = {
  name: "Description list",
  render: () => (
    <DescriptionList className="max-w-2xl">
      <DescriptionTerm>Версия</DescriptionTerm>
      <DescriptionDetails>1.2026.321</DescriptionDetails>
      <DescriptionTerm>Режим</DescriptionTerm>
      <DescriptionDetails>
        <Badge color="green">API-доступ</Badge>
      </DescriptionDetails>
      <DescriptionTerm>Токен</DescriptionTerm>
      <DescriptionDetails>
        Токен команды разработки и исследования пользовательского интерфейса
      </DescriptionDetails>
    </DescriptionList>
  ),
};
export const DataTable: Story = {
  name: "Table",
  render: () => (
    <Table striped>
      <TableHead>
        <TableRow>
          <TableHeader>Агент</TableHeader>
          <TableHeader>Версия</TableHeader>
          <TableHeader>Состояние</TableHeader>
        </TableRow>
      </TableHead>
      <TableBody>
        {["Claude Desktop", "ChatGPT", "Codex CLI"].map((name, i) => (
          <TableRow key={name}>
            <TableCell>{name}</TableCell>
            <TableCell>{["1.0.124", "1.2026.321", "0.112.0"][i]}</TableCell>
            <TableCell>
              <Badge color="green">Готов</Badge>
            </TableCell>
          </TableRow>
        ))}
      </TableBody>
    </Table>
  ),
};
export const Pages: Story = {
  name: "Pagination",
  render: () => (
    <Pagination aria-label="Страницы резервных копий">
      <PaginationPrevious>Назад</PaginationPrevious>
      <PaginationList>
        <PaginationPage href="#page-1" current>
          1
        </PaginationPage>
        <PaginationPage href="#page-2">2</PaginationPage>
        <PaginationGap />
        <PaginationPage href="#page-10">10</PaginationPage>
      </PaginationList>
      <PaginationNext href="#page-2">Далее</PaginationNext>
    </Pagination>
  ),
};
export const Copy: Story = {
  name: "Heading, Text, Link, Divider",
  render: () => (
    <div className="ui-stack max-w-2xl">
      <Heading>Настройки агента</Heading>
      <Subheading>Подключение</Subheading>
      <Text>
        Подключите агента к <Strong>CoreInfra Hub</Strong>. Конфигурация
        хранится локально, например в <Code>config.json</Code>.
      </Text>
      <Divider />
      <Text>
        <TextLink href="#example">Ссылка в тексте</TextLink> подчёркнута и
        доступна с клавиатуры.
      </Text>
      <Link href="#example" className="underline">
        Обычная ссылка
      </Link>
    </div>
  ),
};
