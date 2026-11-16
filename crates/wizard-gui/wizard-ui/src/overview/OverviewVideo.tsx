import { useState } from "react";
import { PlayIcon, XMarkIcon } from "@heroicons/react/20/solid";
import { Button } from "../components/catalyst/button";
import { Dialog, DialogTitle } from "../components/catalyst/dialog";
import { Text } from "../components/catalyst/text";
import poster from "../assets/wizard-demo-poster.webp";

// The demo and poster published on https://coreinfra.ru/wizard.
const wizardVideo = "https://coreinfra.ru/assets/wizard/release-demo-v13.mp4";

export function OverviewVideo() {
  const [open, setOpen] = useState(false);
  const [failed, setFailed] = useState(false);
  const [attempt, setAttempt] = useState(0);
  return <>
    <button type="button" className="overview-video" aria-label="Посмотреть видео о Wizard" onClick={() => { setFailed(false); setOpen(true); }}>
      <span className="overview-video-visual">
        <span className="overview-video-media">
          <img src={poster} alt="" width={1280} height={832} />
        </span>
        <span className="overview-video-play"><PlayIcon /></span>
      </span>
    </button>
    <Dialog open={open} onClose={setOpen} size="4xl">
      <div className="mb-4 flex items-center justify-between gap-4">
        <DialogTitle className="font-semibold!">Wizard в работе</DialogTitle>
        <Button plain aria-label="Закрыть видео" onClick={() => setOpen(false)}><XMarkIcon /></Button>
      </div>
      {open && <video key={attempt} className="w-full rounded-lg bg-black" src={wizardVideo} poster={poster} controls autoPlay playsInline preload="metadata" aria-label="Видео: Wizard в работе" onError={() => setFailed(true)} />}
      {failed && <div className="mt-4 flex flex-wrap items-center justify-between gap-3" role="alert">
        <Text>Не удалось загрузить видео. Проверьте подключение к интернету.</Text>
        <Button outline onClick={() => { setFailed(false); setAttempt((value) => value + 1); }}>Повторить</Button>
      </div>}
    </Dialog>
  </>;
}
