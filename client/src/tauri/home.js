// Tauri port of src/electron/home.js — the home window (recent documents).
import { invoke } from '@tauri-apps/api/core'

let homeApp
let elmWorker

function init (flags) {
  homeApp = window.Elm.Electron.Home.init({ flags })

  homeApp.ports.send.subscribe(([tag, data]) => {
    switch (tag) {
      case 'ClickedNew':
        invoke('new_document')
        break

      case 'ClickedOpen':
        invoke('open_document_dialog')
        break

      case 'ClickedImport':
        clickedImport()
        break

      case 'ClickedDocument':
        invoke('open_document', { path: data })
        break

      case 'ClickedRemoveDocument':
        invoke('remove_recent_document', { path: data })
        break
    }
  })
}

async function clickedImport () {
  const fileData = await invoke('import_json_dialog')
  if (fileData) {
    elmWorker.ports.toElm.send([fileData, true])
  }
}

async function start () {
  const homeState = await invoke('get_home_state')
  homeState.currentTime = Date.now()
  init(homeState)

  // Elm worker, reusing Elm code for JSON import parsing.
  elmWorker = window.Elm.Electron.Worker.init({ flags: Date.now() })
  elmWorker.ports.fromElm.subscribe((elmData) => {
    switch (elmData[0]) {
      case 'ImportDone':
        invoke('import_document', { fileData: elmData[1].data })
        break

      case 'ImportError':
        window.alert('Import Error\n' + elmData[1])
        break
    }
  })
}
start()
