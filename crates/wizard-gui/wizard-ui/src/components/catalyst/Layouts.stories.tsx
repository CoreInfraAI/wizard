import type { Meta, StoryObj } from "@storybook/react-vite";
import { AuthLayout } from "./auth-layout";
import { SidebarLayout } from "./sidebar-layout";
import { StackedLayout } from "./stacked-layout";
import {
  Sidebar,
  SidebarBody,
  SidebarFooter,
  SidebarHeader,
  SidebarHeading,
  SidebarItem,
  SidebarLabel,
  SidebarSection,
} from "./sidebar";
import {
  Navbar,
  NavbarDivider,
  NavbarItem,
  NavbarLabel,
  NavbarSection,
  NavbarSpacer,
} from "./navbar";
import { AppLogo } from "../AppLogo";
import { PageHeader, Panel } from "../ui/Layout";
import { Text } from "./text";
import { Button } from "./button";

const meta = {
  title: "Catalyst/Навигация и каркасы",
  parameters: { layout: "fullscreen" },
} satisfies Meta;
export default meta;
type Story = StoryObj<typeof meta>;
function Navigation() {
  return (
    <Sidebar aria-label="Навигация">
      <SidebarHeader>
        <AppLogo className="h-8 w-auto" />
      </SidebarHeader>
      <SidebarBody>
        <SidebarSection>
          <SidebarItem href="#overview" current>
            <SidebarLabel>Обзор</SidebarLabel>
          </SidebarItem>
        </SidebarSection>
        <SidebarSection>
          <SidebarHeading>Агенты</SidebarHeading>
          <SidebarItem href="#claude">
            <SidebarLabel>Claude Desktop</SidebarLabel>
          </SidebarItem>
          <SidebarItem href="#chatgpt">
            <SidebarLabel>ChatGPT / Codex</SidebarLabel>
          </SidebarItem>
        </SidebarSection>
      </SidebarBody>
      <SidebarFooter>
        <SidebarItem href="#account">
          <SidebarLabel>Личный хаб</SidebarLabel>
        </SidebarItem>
      </SidebarFooter>
    </Sidebar>
  );
}
function Topbar() {
  return (
    <Navbar>
      <NavbarSection>
        <NavbarItem href="#overview" current>
          <NavbarLabel>Обзор</NavbarLabel>
        </NavbarItem>
        <NavbarItem href="#agents">
          <NavbarLabel>Агенты</NavbarLabel>
        </NavbarItem>
      </NavbarSection>
      <NavbarSpacer />
      <NavbarDivider />
      <NavbarItem href="#settings">
        <NavbarLabel>Настройки</NavbarLabel>
      </NavbarItem>
    </Navbar>
  );
}
function Content() {
  return (
    <div className="ui-stack">
      <PageHeader
        title="Рабочее пространство"
        actions={<Button outline>Действие</Button>}
      />
      <Panel title="Содержимое">
        <Text>
          Боковая панель становится меню на узком экране. Отступы и ширина
          контента используют общие токены.
        </Text>
      </Panel>
    </div>
  );
}
export const SidebarNavigation: Story = {
  name: "Sidebar",
  render: () => (
    <div className="h-dvh w-64 border-r border-zinc-200">
      <Navigation />
    </div>
  ),
};
export const TopNavigation: Story = {
  name: "Navbar",
  render: () => (
    <div className="px-4">
      <Topbar />
    </div>
  ),
};
export const SidebarShell: Story = {
  name: "Sidebar layout",
  render: () => (
    <SidebarLayout navbar={null} sidebar={<Navigation />}>
      <Content />
    </SidebarLayout>
  ),
};
export const StackedShell: Story = {
  name: "Stacked layout",
  render: () => (
    <StackedLayout navbar={<Topbar />} sidebar={<Navigation />}>
      <Content />
    </StackedLayout>
  ),
};
export const AuthShell: Story = {
  name: "Auth layout",
  render: () => (
    <AuthLayout>
      <div className="ui-stack max-w-sm">
        <AppLogo className="h-10 w-auto" />
        <PageHeader title="Вход в CoreInfra Hub" />
        <Text>Завершите вход в браузере и вернитесь в Wizard.</Text>
        <Button>Продолжить вход</Button>
      </div>
    </AuthLayout>
  ),
};
