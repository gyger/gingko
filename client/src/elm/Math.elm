module Math exposing (toElements)

{-| Turn LaTeX math in card markdown into `<gw-math>` elements, rendered
by KaTeX on the JS side.

    "a $x^2$ b"  -> "a <gw-math data-tex=\"x&#94;2\">&#36;x&#94;2&#36;</gw-math> b"
    "$$...$$"    -> "<gw-math display data-tex=\"...\">...</gw-math>"

Dollar rules follow pandoc's `tex_math_dollars`, so prices stay text:
the opening `$` is followed by a non-space, the closing `$` is preceded by
a non-space and not followed by a digit. Code spans and fenced code blocks
are left untouched.

-}

import Char
import Regex exposing (Regex)


toElements : String -> String
toElements content =
    content
        |> String.lines
        |> splitFences
        |> List.map convertChunk
        |> String.join "\n"



-- FENCED CODE


type Chunk
    = Text (List String)
    | Code (List String)


{-| Group lines into text and fenced-code chunks, in order.

    [ "a", "```", "$x$", "```", "b" ]
        -> [ Text [ "a" ], Code [ "```", "$x$", "```" ], Text [ "b" ] ]

-}
splitFences : List String -> List Chunk
splitFences lines =
    let
        step line ( chunks, inFence ) =
            let
                isFence =
                    Regex.contains fenceLine line
            in
            case ( chunks, inFence ) of
                ( (Code ls) :: rest, True ) ->
                    ( Code (line :: ls) :: rest, not isFence )

                ( (Text ls) :: rest, False ) ->
                    if isFence then
                        ( Code [ line ] :: chunks, True )

                    else
                        ( Text (line :: ls) :: rest, False )

                _ ->
                    if isFence then
                        ( Code [ line ] :: chunks, True )

                    else
                        ( Text [ line ] :: chunks, False )
    in
    List.foldl step ( [], False ) lines
        |> Tuple.first
        |> List.reverse
        |> List.map reverseChunk


reverseChunk : Chunk -> Chunk
reverseChunk chunk =
    case chunk of
        Text ls ->
            Text (List.reverse ls)

        Code ls ->
            Code (List.reverse ls)


convertChunk : Chunk -> String
convertChunk chunk =
    case chunk of
        Code ls ->
            String.join "\n" ls

        Text ls ->
            Regex.replace mathOrCode replaceMatch (String.join "\n" ls)



-- MATH


{-| Code spans and escaped dollars are matched too, so they are consumed
and returned unchanged instead of being read as math delimiters.
-}
mathOrCode : Regex
mathOrCode =
    regex "(`+)[\\s\\S]*?\\1|\\\\\\$|\\$\\$[\\s\\S]+?\\$\\$|\\$(?=\\S)[^$\\n]*?[^\\s\\\\]\\$(?!\\d)"


fenceLine : Regex
fenceLine =
    regex "^\\s*(```|~~~)"


regex : String -> Regex
regex pattern =
    Regex.fromString pattern |> Maybe.withDefault Regex.never


replaceMatch : Regex.Match -> String
replaceMatch { match } =
    if String.startsWith "$$" match then
        element " display" (String.slice 2 -2 match) match

    else if String.startsWith "$" match then
        element "" (String.slice 1 -1 match) match

    else
        match


{-| The raw source is the fallback content, shown where `gw-math` is not
defined (e.g. the web app).
-}
element : String -> String -> String -> String
element attrs tex source =
    "<gw-math"
        ++ attrs
        ++ " data-tex=\""
        ++ encode tex
        ++ "\">"
        ++ encode source
        ++ "</gw-math>"


{-| Encode every character that markdown or HTML would interpret as a
numeric entity, so marked passes the source through verbatim.

    "a_1 < b" -> "a&#95;1 &#60; b"

-}
encode : String -> String
encode text =
    String.toList text
        |> List.map encodeChar
        |> String.concat


encodeChar : Char -> String
encodeChar c =
    if Char.isAlphaNum c || c == ' ' then
        String.fromChar c

    else
        "&#" ++ String.fromInt (Char.toCode c) ++ ";"
