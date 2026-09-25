package main

import "fmt"

// RunStorageCases executes a save corpus selection through its registered
// routes and returns one executed observation per declared checkpoint.
func RunStorageCases(root string, selection Inventory, routes map[ConsumerRoute]GoOperation) ([]ExecutedObservation, error) {
	if len(selection.Cases) == 0 {
		return nil, fmt.Errorf("runtime-oracle: manifest selection is empty")
	}
	if len(routes) == 0 {
		return nil, fmt.Errorf("runtime-oracle: no registered storage routes")
	}
	return runCasesByRoute(root, selection, func(family, version, operation string) (GoOperation, error) {
		route := ConsumerRoute{FamilyID: family, Version: version, Operation: operation}
		producer, registered := routes[route]
		if !registered {
			return nil, fmt.Errorf("route %s/%s/%s has no registered Go producer", family, version, operation)
		}
		return producer, nil
	})
}
