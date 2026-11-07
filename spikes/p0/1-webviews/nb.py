import marimo

__generated_with = "0.25.2"
app = marimo.App()


@app.cell
def _():
    import marimo as mo

    return (mo,)


@app.cell
def _(mo):
    mo.md("""
    # 标题 Heading

    正文：研究树、实验、对比。Body text `code => ->`
    """)
    return


@app.cell
def _():
    x = sum(range(10))
    x
    return


if __name__ == "__main__":
    app.run()
