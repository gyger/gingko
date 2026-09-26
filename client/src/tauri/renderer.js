// Tauri port of src/electron/renderer.js — the document window.
import { invoke } from '@tauri-apps/api/core'
import { getCurrentWindow } from '@tauri-apps/api/window'
import commitTree from './commit.js'

const Mousetrap = require('mousetrap')
const helpers = require('../shared/doc-helpers')

// Init Vars

window.elmMessages = []
let lastActivesScrolled = null
let lastColumnScrolled = null
let ticking = false
let DIRTY = false
let isUntitled = false
let closing = false
const savedImmutables = new Set()
const GIT_LIKE_DATA = Symbol.for('couchdb')
const getDataType = () => GIT_LIKE_DATA

const localStore = {
  isReady: () => { return true },
  set: (key, value) => {
    invoke('local_store_set', { key, value })
  }
}

let gingkoElectron

const objectsToElmData = (objs) => {
  const groups = {}
  for (const obj of objs) {
    const key = Object.prototype.hasOwnProperty.call(obj, 'type') ? obj.type : obj._id
    if (!groups[key]) { groups[key] = [] }
    groups[key].push(obj)
  }
  return groups
}

/* ==== Startup: fetch document state from the backend ==== */

async function start () {
  const docState = await invoke('get_doc_state')
  DIRTY = docState.fileData === null
  isUntitled = docState.isUntitled

  const timestamp = Date.now()
  gingkoElectron = window.Elm.Electron.Electron.init({
    flags: {
      filePath: docState.filePath,
      fileData: docState.fileData,
      fileSettings: docState.fileSettings,
      undoData: objectsToElmData(docState.undoData),
      globalData: {
        seed: timestamp,
        currentTime: timestamp,
        isMac: navigator.platform.toUpperCase().indexOf('MAC') >= 0
      },
      isUntitled
    }
  })

  gingkoElectron.ports.infoForOutside.subscribe((elmdata) => {
    fromElm(elmdata.tag, elmdata.data)
  })
}
start()

/* ==== Menu events from the backend ==== */

const currentWindow = getCurrentWindow()

currentWindow.listen('menu-clicked', async (event) => {
  switch (event.payload) {
    case 'menu:save':
    case 'menu:saveas':
      await saveThisAs()
      break

    case 'menu:export':
      toElm(null, 'docMsgs', 'ClickedExport')
      break

    case 'menu:undo':
      toElm('mod+z', 'docMsgs', 'Keyboard')
      break

    case 'menu:cut':
      toElm('mod+x', 'docMsgs', 'Keyboard')
      break

    case 'menu:copy':
      toElm('mod+c', 'docMsgs', 'Keyboard')
      break

    case 'menu:paste':
      toElm('mod+v', 'docMsgs', 'Keyboard')
      break

    case 'menu:pasteinto':
      toElm('mod+shift+v', 'docMsgs', 'Keyboard')
      break
  }
})

async function saveThisAs () {
  const newPath = await invoke('save_file_dialog')
  if (newPath) {
    const [savedPath, timestamp, untitled] = await invoke('save_as', { newPath })
    DIRTY = false
    isUntitled = untitled
    toElm([savedPath, timestamp], 'docMsgs', 'SavedToFile')
  }
}

/* ==== Window close handling ==== */

currentWindow.onCloseRequested(async (event) => {
  if (closing) { return }
  if (isUntitled) {
    event.preventDefault()
    const answer = await invoke('ask_save_changes')
    switch (answer) {
      case 'discard':
        closing = true
        await invoke('close_document')
        break

      case 'save':
        await saveThisAs()
        if (!isUntitled) {
          closing = true
          await invoke('close_document')
        }
        break
    }
  } else if (DIRTY) {
    // A local save is triggered on every change; give it a moment to land.
    event.preventDefault()
    setTimeout(async () => {
      closing = true
      await invoke('close_document')
    }, 200)
  }
  // Otherwise: let the close proceed; the backend cleans up on Destroyed.
})

window.checkboxClicked = (cardId, number) => {
  toElm([cardId, number], 'docMsgs', 'CheckboxClicked')
}

/* === Elm / JS Interop === */

const fromElm = (msg, elmData) => {
  window.elmMessages.push({ tag: msg, data: elmData })
  window.elmMessages = window.elmMessages.slice(-10)

  const casesTauri = {
    Alert: () => {
      window.alert(elmData)
    },

    SetDirty: () => {
      DIRTY = elmData
    },

    DragStart: () => {
      const cardElement = elmData.target.parentElement
      const cardId = cardElement.id.replace(/^card-/, '')
      elmData.dataTransfer.setDragImage(cardElement, 0, 0)
      elmData.dataTransfer.setData('text', '')
      toElm(cardId, 'docMsgs', 'DragStarted')
    },

    DragDone: () => {},

    SendCollabState: () => {},

    ScrollFullscreenCards: () => {
      helpers.scrollFullscreen(elmData)
    },

    CommitData: async () => {
      const [commitSha, objects] = await commitTree(
        elmData.author, elmData.parents, elmData.workingTree, Date.now(), elmData.metadata
      )
      objects.push({ _id: 'heads/master', type: 'ref', value: commitSha, ancestors: [], _rev: '' })

      const toPersist = objects.filter((o) => !savedImmutables.has(o._id))
      try {
        await invoke('commit_data', { objects: toPersist })
        toPersist.forEach((o) => {
          if (o._id !== 'heads/master') { savedImmutables.add(o._id) }
        })
        toElm(objectsToElmData(objects), 'docMsgs', 'DataSaved')
      } catch (e) {
        console.error(e)
      }
    },

    ExportToFile: async () => {
      const format = elmData[0]
      const content = elmData[1]
      const filePath = await invoke('export_file_dialog', { format })
      if (filePath) {
        try {
          if (format === 'docx') {
            await invoke('export_docx', { path: filePath, content })
          } else {
            await invoke('export_file', { path: filePath, content })
          }
        } catch (e) {
          window.alert('Failed to Export\n' + e.toString())
        }
      }
    },

    SaveToFile: async () => {
      try {
        const [filePath, timestamp, untitled] = await invoke('save_file', { data: elmData[1] })
        DIRTY = false
        isUntitled = untitled
        toElm([filePath, timestamp], 'docMsgs', 'SavedToFile')
      } catch (e) {
        console.error(e)
      }
    }
  }

  const params = { localStore, lastColumnScrolled, lastActivesScrolled, ticking, DIRTY }

  const cases = Object.assign(helpers.casesShared(elmData, params), casesTauri)

  try {
    cases[msg]()
  } catch (err) {
    console.error('Unexpected message from Elm : ', msg, elmData, err)
  }
}

function toElm (data, portName, tagName) {
  if (!gingkoElectron) { return }
  const portExists = Object.prototype.hasOwnProperty.call(gingkoElectron.ports, portName)
  const tagGiven = typeof tagName === 'string'

  if (portExists) {
    var dataToSend

    if (tagGiven) {
      dataToSend = { tag: tagName, data: data }
    } else {
      dataToSend = data
    }
    gingkoElectron.ports[portName].send(dataToSend)
  } else {
    console.error('Unknown port', portName, data)
  }
}

/* === Custom textareas === */

helpers.defineCustomTextarea(toElm, getDataType)

/* === Keyboard === */

const desktopShortcuts = helpers.shortcuts
  .filter((x) => x !== 'mod+s' && x !== 'mod+o')

Mousetrap.bind(desktopShortcuts, function (e, s) {
  switch (s) {
    case 'enter':
      if (document.activeElement.nodeName === 'TEXTAREA') {
        return
      } else {
        toElm('enter', 'docMsgs', 'Keyboard')
      }
      break

    case 'mod+c': {
      const exportPreview = document.getElementById('export-preview')
      if (exportPreview !== null) {
        return
      } else {
        toElm('mod+c', 'docMsgs', 'Keyboard')
      }
      break
    }

    case 'mod+v':
    case 'mod+shift+v': {
      const elmTag = s === 'mod+v' ? 'Paste' : 'PasteInto'

      navigator.clipboard.readText()
        .then(clipString => {
          try {
            const clipObj = JSON.parse(clipString)
            toElm(clipObj, 'docMsgs', elmTag)
          } catch {
            toElm(clipString, 'docMsgs', elmTag)
          }
        })
      break
    }

    case 'alt+0':
    case 'alt+1':
    case 'alt+2':
    case 'alt+3':
    case 'alt+4':
    case 'alt+5':
    case 'alt+6':
      if (document.activeElement.nodeName === 'TEXTAREA') {
        const num = Number(s[s.length - 1])
        const currentText = document.activeElement.value
        const newText = currentText.replace(/^(#{0,6}) ?(.*)/, num === 0 ? '$2' : '#'.repeat(num) + ' $2')
        document.activeElement.value = newText
        DIRTY = true
        toElm(newText, 'docMsgs', 'FieldChanged')

        const cardElementId = document.activeElement.id.replace(/^card-edit/, 'card')
        const card = document.getElementById(cardElementId)
        if (card !== null) {
          card.dataset.clonedContent = newText
        }
      }
      break

    default:
      toElm(s, 'docMsgs', 'Keyboard')
  }

  if (helpers.needOverride.includes(s)) {
    return false
  }
})
