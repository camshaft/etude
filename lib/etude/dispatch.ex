defmodule Etude.Dispatch do
  defmacro __using__(_) do
    quote do
      import unquote(__MODULE__.Helpers)
      @before_compile unquote(__MODULE__)

      def resolve(module, function, arity) do
        lookup(module, function, arity)
      end
      defoverridable resolve: 3
    end
  end

  def from_process do
    Process.get(:__ETUDE_DISPATCH__, Etude.Dispatch.Fallback)
  end

  defmacro __before_compile__(_) do
    quote do
      defp lookup(module, function, nil) do
        %Etude.Thunk.Continuation{
          function: fn(arguments, state) ->
            continuation = resolve(module, function, length(arguments))
            {%{continuation | arguments: arguments}, state}
          end
        }
      end
      defp lookup(module, function, arity) do
        thunk = function_exported?(module, :__etude__, 3) && module.__etude__(function, arity, __MODULE__)
        thunk || Etude.Dispatch.eager_apply(module, function, arity)
      end
    end
  end

  args = fn
    (0) ->
      []
    (arity) ->
      for arg <- 1..arity do
        Macro.var(:"arg_#{arg}", nil)
      end
  end

  for arity <- 0..48 do
    args = args.(arity)

    def eager_apply(module, function, unquote(arity)) do
      %Etude.Thunk.Continuation{
        function: fn(arguments, state) ->
          Etude.Thunk.resolve_all(arguments, state, fn(arguments = unquote(args), state) ->
            Etude.Cache.memoize(state, {module, function, arguments}, fn() ->
              apply(module, function, unquote(args))
            end)
          end)
        end,
        arguments: unquote(args |> Enum.map(fn(_) -> nil end))
      }
    end
  end
end
