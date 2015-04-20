defmodule Etude.Node.Collection do
  alias Etude.Children
  import Etude.Vars

  defprotocol Construction do
    def construct(node, vars)  
  end

  def compile(node, opts) do
    name = Etude.Node.name(node, opts)
    exec = "#{name}_exec" |> String.to_atom

    quote do
      @compile {:nowarn_unused_function, {unquote(name), unquote(length(op_args))}}
      @compile {:inline, [{unquote(name), unquote(length(op_args))}]}
      defp unquote(name)(unquote_splicing(op_args)) do
        Etude.Memoize.wrap unquote(name) do
          ## dependencies
          unquote_splicing(Children.call(node, opts))

          ## construction
          case unquote(exec)(unquote_splicing(Children.vars(node, opts))) do
            nil ->
              Logger.debug(unquote("#{name} deps_pending"))
              {nil, unquote(state)}
            val ->
              Logger.debug(fn -> unquote("#{name} result -> ") <> inspect(elem(val, 1)) end)
              {val, unquote(state)}
          end
        end
      end

      unquote_splicing(compile_exec(exec, node, opts))
      unquote_splicing(Children.compile(node, opts))
    end
  end

  defp compile_exec(name, node, opts) do
    construction = Construction.construct(node, Children.vars(node, opts))
    case Children.count(node) do
      0 ->
        [quote do
          @compile {:inline, [{unquote(name), 0}]}
          defp unquote(name)() do
            {unquote(Etude.Utils.ready),
             unquote(construction)}
          end
        end]
      count ->
        elem(quote do
          @compile {:inline, [{unquote(name), unquote(count)}]}
          defp unquote(name)(unquote_splicing(Children.args(node, opts))) do
            {unquote(Etude.Utils.ready),
             unquote(construction)}
          end
          defp unquote(name)(unquote_splicing(Children.wildcard(node, opts))) do
            nil
          end
        end, 2)
    end
  end
end