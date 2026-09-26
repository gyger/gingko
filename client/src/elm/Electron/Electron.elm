module Electron.Electron exposing (..)

import Browser
import Browser.Dom exposing (Element)
import Coders exposing (treeToMarkdownOutline)
import Doc.Data as Data
import Doc.Fullscreen exposing (viewFullscreenButtonsDesktop)
import Doc.History as History exposing (History)
import Doc.TreeStructure as TreeStructure exposing (Msg(..))
import Doc.UI as UI
import GlobalData
import Html exposing (Html, div, text)
import Html.Attributes exposing (classList, id, title)
import Html.Lazy exposing (lazy5)
import Json.Decode as Dec exposing (Value)
import Json.Encode as Enc
import Outgoing exposing (Msg(..), send)
import Page.Doc exposing (MsgToParent(..))
import Page.Doc.Export as Export exposing (ExportFormat(..), ExportSelection(..), exportView, toExtension)
import Page.Doc.Incoming as Incoming exposing (Msg(..))
import Page.Doc.Theme exposing (Theme(..), applyTheme)
import Task
import Time
import Translation exposing (Language, TranslationId, timeDistInWords)
import Types exposing (Children(..), TooltipPosition, Tree, ViewMode(..))
import UI.Header exposing (viewExportMenu)


main : Program DataIn Model Msg
main =
    Browser.document
        { init = init
        , update = update
        , view = \m -> Browser.Document "Gingko Writer Desktop" (view m)
        , subscriptions = subscriptions
        }



-- MODEL


type alias Model =
    { docModel : Page.Doc.Model
    , data : Data.Model
    , fileState : FileState
    , lastSave : Time.Posix
    , uiState : UIState
    , tooltip : Maybe ( Element, TooltipPosition, TranslationId )
    , theme : Theme
    }


type FileState
    = UntitledFileDoc String
    | FileDoc String


type UIState
    = DocUI
    | VersionHistoryView History
    | ExportPreview ( ExportSelection, ExportFormat )


fileStateToPath : FileState -> String
fileStateToPath fState =
    case fState of
        UntitledFileDoc str ->
            str

        FileDoc str ->
            str


type alias DataIn =
    { filePath : String
    , fileData : Maybe String
    , fileSettings : Value
    , undoData : Value
    , globalData : Value
    , isUntitled : Bool
    }


init : DataIn -> ( Model, Cmd Msg )
init dataIn =
    let
        globalData =
            GlobalData.decode dataIn.globalData

        lastActivesResult =
            Dec.decodeValue (Dec.field "last-actives" (Dec.list Dec.string)) dataIn.fileSettings

        undoData =
            Data.success dataIn.undoData Data.empty

        -- The default tree is empty; the web app creates the first card via the
        -- database flow, so the desktop wrapper has to seed card "1" itself
        -- (init True starts in Editing mode on card "1").
        newDocTree =
            Tree "0" "" (Children [ Tree "1" "" (Children []) ])

        ( initDocModel, initFileState, maybeFocus ) =
            case dataIn.fileData of
                Nothing ->
                    ( Page.Doc.init True globalData
                        |> setDocTree newDocTree
                    , UntitledFileDoc dataIn.filePath
                    , Task.attempt (always NoOp) (Browser.Dom.focus "card-edit-1")
                    )

                Just fileData ->
                    case Coders.normalizeAndParse fileData of
                        Ok parsedTrees ->
                            let
                                -- Files without <gingko-card> tags (e.g. plain
                                -- markdown) parse to zero cards; load the whole
                                -- content as a single card instead.
                                loadedTree =
                                    if List.isEmpty parsedTrees then
                                        Tree "0" "" (Children [ Tree "1" fileData (Children []) ])

                                    else
                                        Tree "0" "" (Children parsedTrees)
                            in
                            ( Page.Doc.init False globalData
                                |> setDocTree loadedTree
                                |> Page.Doc.setLoading False
                            , if dataIn.isUntitled then
                                UntitledFileDoc dataIn.filePath

                              else
                                FileDoc dataIn.filePath
                            , Cmd.none
                            )

                        Err _ ->
                            ( Page.Doc.init True globalData
                                |> setDocTree newDocTree
                            , UntitledFileDoc "parser error"
                            , Task.attempt (always NoOp) (Browser.Dom.focus "card-edit-1")
                            )

        maybeLocalSave ( m, c ) =
            if dataIn.isUntitled && dataIn.fileData /= Nothing then
                localSaveDo ( m, c )

            else
                ( m, c )

        ( updatedDocModel, activateCmd ) =
            Page.Doc.lastActives lastActivesResult initDocModel
    in
    ( { docModel = updatedDocModel
      , data = undoData
      , fileState = initFileState
      , lastSave = GlobalData.currentTime globalData
      , uiState = DocUI
      , tooltip = Nothing
      , theme = Default
      }
    , Cmd.batch [ maybeFocus, Cmd.map GotDocMsg activateCmd ]
    )
        |> maybeLocalSave


setDocTree : Tree -> Page.Doc.Model -> Page.Doc.Model
setDocTree tree docModel =
    Page.Doc.setWorkingTree
        (TreeStructure.setTree tree (Page.Doc.getWorkingTree docModel))
        docModel



-- UPDATE


type Msg
    = NoOp
    | GotDocMsg Page.Doc.Msg
      --
    | HistoryToggled Bool
    | CheckoutCommit String
    | Restore
      --
    | CloseExport
    | ExportFormatChanged ExportFormat
    | ExportSelectionChanged ExportSelection
    | Export
      --
    | TooltipRequested String TooltipPosition TranslationId
    | TooltipReceived Element TooltipPosition TranslationId
    | TooltipClosed
      --
    | ExitFullscreenRequested
    | SaveAndExitFullscreen
      --
    | TimeUpdate Time.Posix
    | Incoming Incoming.Msg
    | LogErr String


update : Msg -> Model -> ( Model, Cmd Msg )
update msg ({ docModel } as model) =
    case msg of
        GotDocMsg docMsg ->
            let
                ( newDocModel, newCmd, parentMsgs ) =
                    Page.Doc.opaqueUpdate docMsg docModel
                        |> (\( m, c, p ) -> ( m, Cmd.map GotDocMsg c, p ))
            in
            ( { model | docModel = newDocModel }, newCmd )
                |> applyParentMsgs parentMsgs

        HistoryToggled isOpen ->
            if isOpen then
                openHistorySlider model

            else
                -- Cancel history browsing: revert to the original tree
                case model.uiState of
                    VersionHistoryView history ->
                        case History.revert history of
                            Just origTree ->
                                let
                                    ( newDocModel, docCmd, _ ) =
                                        Page.Doc.setTree origTree docModel
                                in
                                ( { model | docModel = newDocModel, uiState = DocUI }
                                , Cmd.map GotDocMsg docCmd
                                )

                            Nothing ->
                                ( { model | uiState = DocUI }, Cmd.none )

                    _ ->
                        ( { model | uiState = DocUI }, Cmd.none )

        CheckoutCommit commitSha ->
            case model.uiState of
                VersionHistoryView history ->
                    case History.checkoutVersion commitSha history of
                        Just ( newHistory, newTree ) ->
                            let
                                ( newDocModel, docCmd, _ ) =
                                    Page.Doc.setTree newTree docModel

                                ( activatedDocModel, activateCmd ) =
                                    Page.Doc.maybeActivate newDocModel
                            in
                            ( { model
                                | docModel = activatedDocModel
                                , uiState = VersionHistoryView newHistory
                              }
                            , Cmd.map GotDocMsg (Cmd.batch [ docCmd, activateCmd ])
                            )

                        Nothing ->
                            ( model, Cmd.none )

                _ ->
                    ( model, Cmd.none )

        Restore ->
            ( { model | uiState = DocUI }
            , Cmd.none
            )
                |> localSaveDo
                |> addToHistoryDo

        --
        CloseExport ->
            case model.uiState of
                ExportPreview _ ->
                    ( { model | uiState = DocUI }, Cmd.none )

                _ ->
                    ( model, Cmd.none )

        ExportFormatChanged newExpFormat ->
            case model.uiState of
                ExportPreview ( oldExpSelection, _ ) ->
                    ( { model | uiState = ExportPreview ( oldExpSelection, newExpFormat ) }, Cmd.none )

                _ ->
                    ( model, Cmd.none )

        ExportSelectionChanged newExpSelection ->
            case model.uiState of
                ExportPreview ( _, oldExpFormat ) ->
                    ( { model | uiState = ExportPreview ( newExpSelection, oldExpFormat ) }, Cmd.none )

                _ ->
                    ( model, Cmd.none )

        Export ->
            case ( model.uiState, Page.Doc.getActiveTree docModel ) of
                ( ExportPreview ( expSel, expFormat ), Just activeTree ) ->
                    ( model
                    , send <|
                        ExportToFile (toExtension expFormat)
                            (Export.toString (fileStateToPath model.fileState)
                                ( expSel, expFormat )
                                activeTree
                                (Page.Doc.getWorkingTree docModel).tree
                            )
                    )

                _ ->
                    ( model, Cmd.none )

        TimeUpdate newTime ->
            let
                newGlobalData =
                    Page.Doc.getGlobalData docModel
                        |> GlobalData.updateTime newTime
            in
            ( { model | docModel = Page.Doc.setGlobalData newGlobalData docModel }, Cmd.none )

        Incoming incomingMsg ->
            let
                passthrough =
                    Page.Doc.opaqueIncoming incomingMsg docModel
                        |> (\( d, c, p ) ->
                                ( { model | docModel = d }, Cmd.map GotDocMsg c )
                                    |> applyParentMsgs p
                           )
            in
            case incomingMsg of
                SavedToFile newPath savedTime ->
                    let
                        oldPath =
                            fileStateToPath model.fileState

                        newGlobalData =
                            Page.Doc.getGlobalData docModel
                                |> GlobalData.updateTime savedTime

                        newDocModel =
                            docModel
                                |> Page.Doc.setDirty False
                                |> Page.Doc.setGlobalData newGlobalData
                    in
                    if newPath /= oldPath then
                        ( { model | fileState = FileDoc newPath, docModel = newDocModel, lastSave = savedTime }, Cmd.none )

                    else
                        ( { model | docModel = newDocModel, lastSave = savedTime }, Cmd.none )

                DataSaved dataIn ->
                    let
                        newData =
                            Data.success dataIn model.data

                        newUiState =
                            case model.uiState of
                                VersionHistoryView history ->
                                    VersionHistoryView (History.update newData history)

                                other ->
                                    other
                    in
                    ( { model
                        | data = newData
                        , uiState = newUiState
                        , docModel = Page.Doc.setDirty False docModel
                      }
                    , Cmd.none
                    )

                ClickedExport ->
                    ( { model | uiState = ExportPreview ( ExportEverything, DOCX ) }, Cmd.none )

                Keyboard "mod+z" ->
                    case Page.Doc.getViewMode docModel of
                        Normal _ ->
                            openHistorySlider model

                        _ ->
                            passthrough

                _ ->
                    passthrough

        LogErr err ->
            ( model, send (ConsoleLogRequested err) )

        TooltipRequested elId tipPos content ->
            ( model
            , Browser.Dom.getElement elId
                |> Task.attempt
                    (\result ->
                        case result of
                            Ok el ->
                                TooltipReceived el tipPos content

                            Err _ ->
                                NoOp
                    )
            )

        TooltipReceived el tipPos content ->
            ( { model | tooltip = Just ( el, tipPos, content ) }, Cmd.none )

        TooltipClosed ->
            ( { model | tooltip = Nothing }, Cmd.none )

        --
        ExitFullscreenRequested ->
            Page.Doc.opaqueIncoming (Keyboard "esc") docModel
                |> (\( d, c, p ) ->
                        ( { model | docModel = d }, Cmd.map GotDocMsg c )
                            |> applyParentMsgs p
                   )

        SaveAndExitFullscreen ->
            Page.Doc.opaqueIncoming (Keyboard "mod+enter") docModel
                |> (\( d, c, p ) ->
                        ( { model | docModel = d }, Cmd.map GotDocMsg c )
                            |> applyParentMsgs p
                   )

        NoOp ->
            ( model, Cmd.none )


applyParentMsgs : List MsgToParent -> ( Model, Cmd Msg ) -> ( Model, Cmd Msg )
applyParentMsgs parentMsgs tuple =
    List.foldl applyParentMsg tuple parentMsgs


applyParentMsg : MsgToParent -> ( Model, Cmd Msg ) -> ( Model, Cmd Msg )
applyParentMsg parentMsg ( model, prevCmd ) =
    case parentMsg of
        ParentAddToast _ _ ->
            ( model, prevCmd )

        CloseTooltip ->
            ( { model | tooltip = Nothing }, prevCmd )

        OpenAIPrompt ->
            ( model, prevCmd )

        LocalSave _ ->
            localSaveDo ( model, prevCmd )

        Commit ->
            addToHistoryDo ( model, prevCmd )


localSaveDo : ( Model, Cmd Msg ) -> ( Model, Cmd Msg )
localSaveDo mcTuple =
    sendSaveMsg SaveToFile mcTuple


sendSaveMsg : (String -> String -> Outgoing.Msg) -> ( Model, Cmd Msg ) -> ( Model, Cmd Msg )
sendSaveMsg msg ( { fileState } as model, prevCmd ) =
    let
        workingTree =
            Page.Doc.getWorkingTree model.docModel

        treeToSave =
            case Page.Doc.getViewMode model.docModel of
                Normal _ ->
                    workingTree.tree

                Editing { cardId, field } ->
                    TreeStructure.update (Upd cardId field) workingTree
                        |> .tree

                FullscreenEditing { cardId, field } ->
                    TreeStructure.update (Upd cardId field) workingTree
                        |> .tree
    in
    ( model
    , Cmd.batch
        [ send <| msg (fileStateToPath fileState) (treeToMarkdownOutline False treeToSave)
        , prevCmd
        ]
    )


addToHistoryDo : ( Model, Cmd Msg ) -> ( Model, Cmd Msg )
addToHistoryDo ( { docModel, fileState } as model, prevCmd ) =
    let
        author =
            "<local-file>"

        metadata =
            fileStateToPath fileState
                |> Enc.string

        commitReq_ =
            Data.requestCommit (Page.Doc.getWorkingTree docModel).tree author model.data metadata
    in
    case commitReq_ of
        Just commitReq ->
            ( model
            , Cmd.batch
                [ send <| CommitData commitReq
                , prevCmd
                ]
            )

        Nothing ->
            ( model, prevCmd )


openHistorySlider : Model -> ( Model, Cmd Msg )
openHistorySlider model =
    let
        history =
            History.init (Page.Doc.getWorkingTree model.docModel).tree model.data
    in
    case History.getCurrentVersionId history of
        Just _ ->
            ( { model | uiState = VersionHistoryView history }, Cmd.none )

        Nothing ->
            ( model, Cmd.none )



-- VIEW


view : Model -> List (Html Msg)
view ({ docModel } as model) =
    let
        globalData =
            Page.Doc.getGlobalData docModel

        lang =
            GlobalData.language globalData

        activeTree_ =
            Page.Doc.getActiveTree docModel

        isFullscreen =
            Page.Doc.isFullscreen docModel

        isDirty =
            Page.Doc.isDirty docModel

        exportViewOk expSettings =
            lazy5 exportView
                { export = Export
                , printRequested = NoOp
                , tooltipRequested = TooltipRequested
                , tooltipClosed = TooltipClosed
                }
                (fileStateToPath model.fileState)
                expSettings

        maybeExportView expSettings =
            case activeTree_ of
                Just activeTree ->
                    exportViewOk expSettings activeTree (Page.Doc.getWorkingTree docModel).tree

                Nothing ->
                    text ""

        viewTooltip =
            case model.tooltip of
                Just tooltip ->
                    UI.viewTooltip lang tooltip

                Nothing ->
                    text ""
    in
    [ div [ id "desktop-root", applyTheme model.theme ]
        ([ viewFileSaveIndicator
            { language = lang
            , dirty = isDirty
            , isFullscreen = isFullscreen
            , lastSave = model.lastSave
            , currentTime = GlobalData.currentTime globalData
            }
         ]
            ++ Page.Doc.view
                { docMsg = GotDocMsg
                , keyboard = \s -> Incoming (Keyboard s)
                , tooltipRequested = TooltipRequested
                , tooltipClosed = TooltipClosed
                }
                (Just model.lastSave)
                (Just model.lastSave)
                docModel
            ++ (case model.uiState of
                    DocUI ->
                        []

                    VersionHistoryView history ->
                        [ History.view
                            { lang = lang
                            , noOp = NoOp
                            , checkoutTree = CheckoutCommit
                            , restore = Restore
                            , cancel = HistoryToggled False
                            , tooltipRequested = TooltipRequested
                            , tooltipClosed = TooltipClosed
                            }
                            history
                        ]

                    ExportPreview exportSettings ->
                        [ viewExportMenu lang
                            { exportFormatChanged = ExportFormatChanged
                            , exportSelectionChanged = ExportSelectionChanged
                            , tooltipRequested = TooltipRequested
                            , tooltipClosed = TooltipClosed
                            , toggledExport = CloseExport
                            }
                            True
                            exportSettings
                        , maybeExportView exportSettings
                        ]
               )
            ++ (if isFullscreen then
                    viewFullscreenButtonsDesktop
                        { exitFullscreenRequested = ExitFullscreenRequested
                        , saveAndExitFullscreen = SaveAndExitFullscreen
                        }
                        { isMac = GlobalData.isMac globalData
                        , dirty = isDirty
                        }
                        |> List.singleton

                else
                    []
               )
            ++ [ viewTooltip ]
        )
    ]


viewFileSaveIndicator : { language : Language, dirty : Bool, isFullscreen : Bool, lastSave : Time.Posix, currentTime : Time.Posix } -> Html msg
viewFileSaveIndicator { language, dirty, isFullscreen, lastSave, currentTime } =
    let
        lastSaveInWords =
            if abs (Time.posixToMillis lastSave - Time.posixToMillis currentTime) < 3000 then
                "Just now"

            else
                timeDistInWords language lastSave currentTime
    in
    div
        [ id "file-save-indicator"
        , classList [ ( "dirty", dirty ), ( "fullscreen", isFullscreen ) ]
        , title lastSaveInWords
        ]
        [ text <|
            if dirty then
                "Unsaved changes..."

            else
                "All Changes Saved"
        ]



-- SUBSCRIPTIONS


subscriptions : Model -> Sub Msg
subscriptions _ =
    Sub.batch
        [ Incoming.subscribe Incoming LogErr
        , Time.every (9 * 1000) TimeUpdate
        ]
