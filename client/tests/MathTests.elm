module MathTests exposing (suite)

import Expect
import Math exposing (toElements)
import Test exposing (..)


suite : Test
suite =
    describe "Math.toElements"
        [ test "inline math" <|
            \_ ->
                toElements "a $x^2$ b"
                    |> Expect.equal "a <gw-math data-tex=\"x&#94;2\">&#36;x&#94;2&#36;</gw-math> b"
        , test "display math, may span lines" <|
            \_ ->
                toElements "$$\na_1\n$$"
                    |> Expect.equal "<gw-math display data-tex=\"&#10;a&#95;1&#10;\">&#36;&#36;&#10;a&#95;1&#10;&#36;&#36;</gw-math>"
        , test "markdown and html characters are encoded" <|
            \_ ->
                toElements "$a_1 * b_2 < c$"
                    |> Expect.equal "<gw-math data-tex=\"a&#95;1 &#42; b&#95;2 &#60; c\">&#36;a&#95;1 &#42; b&#95;2 &#60; c&#36;</gw-math>"
        , test "prices stay text" <|
            \_ ->
                toElements "costs $5 and $10 today"
                    |> Expect.equal "costs $5 and $10 today"
        , test "closing dollar followed by digit is not math" <|
            \_ ->
                toElements "$x$5"
                    |> Expect.equal "$x$5"
        , test "space after opening dollar is not math" <|
            \_ ->
                toElements "$ x$"
                    |> Expect.equal "$ x$"
        , test "escaped dollar stays text" <|
            \_ ->
                toElements "\\$x$"
                    |> Expect.equal "\\$x$"
        , test "inline code is skipped" <|
            \_ ->
                toElements "`$x$` and $y$"
                    |> Expect.equal "`$x$` and <gw-math data-tex=\"y\">&#36;y&#36;</gw-math>"
        , test "fenced code is skipped" <|
            \_ ->
                toElements "```\n$x$\n```\n$y$"
                    |> Expect.equal "```\n$x$\n```\n<gw-math data-tex=\"y\">&#36;y&#36;</gw-math>"
        ]
